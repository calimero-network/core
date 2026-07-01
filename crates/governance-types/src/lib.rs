//! Signed group operations for **local** governance (no chain).
//!
//! See `docs/context-management/LOCAL-GROUP-GOVERNANCE.md`.
//!
//! ## Provenance
//!
//! Previously located at `calimero_context_client::local_governance`.
//! Extracted to its own leaf crate (#2479, part of epic #2300) so the
//! to-be-created `calimero-governance-store` (#2307) can depend on
//! these op types without transitively re-acquiring `actix` through
//! `calimero-context-client`. A `#[deprecated]` re-export shim at the
//! old path keeps existing callers compiling.
//!
//! ## Namespace governance model
//!
//! A **namespace** has a single governance DAG. Operations in the DAG are
//! either *root ops* (cleartext, visible to all namespace members) or
//! *group-scoped ops* (tagged with a cleartext `group_id` for routing, but
//! the payload is encrypted with the group's sender key so only group
//! members can read it). Non-members store an opaque **skeleton** for
//! group-scoped ops they cannot decrypt.

use std::collections::BTreeMap;
use std::io;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_context_config::types::{AppKey, SignedGroupOpenInvitation};
use calimero_context_config::{MemberCapabilities, VisibilityMode};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::logical_clock::HybridTimestamp;
use ed25519_dalek::SignatureError;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub mod wire;
pub use wire::{
    hash_scoped_group, hash_scoped_namespace, GroupTopicMsg, NamespaceTopicMsg, ReadinessProbe,
    SignedAck, SignedMigrationHeartbeat, SignedReadinessBeacon,
};

// `AckRouter` and its pub/sub plumbing stay in `calimero-context-client`
// (they depend on `tokio::sync::broadcast` and connect actor mailboxes).
// This crate is pure-data: types, signing, hashing, borsh layout.

/// Define a 32-byte id newtype. Borsh-transparent (a derived `[u8; 32]`, no tag
/// or length prefix) so it is byte-compatible with the bare `[u8; 32]` these
/// ids used to be, while making the distinct id kinds non-interchangeable.
macro_rules! id_newtype {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(
            Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash,
            BorshSerialize, BorshDeserialize,
        )]
        pub struct $name([u8; 32]);

        impl $name {
            #[must_use]
            pub const fn new(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }
            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }
            #[must_use]
            pub const fn to_bytes(self) -> [u8; 32] {
                self.0
            }
        }

        impl From<[u8; 32]> for $name {
            fn from(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }
        }

        impl From<$name> for [u8; 32] {
            fn from(id: $name) -> Self {
                id.0
            }
        }
    };
}

id_newtype! {
    /// Stable id of a namespace (the root governance group of a DAG).
    NamespaceId
}
id_newtype! {
    /// Stable id of a group/namespace encryption key.
    KeyId
}

/// Wire/schema version for [`SignedGroupOp`].
///
/// v5: Drop the `cut: GovernanceParentEdge` field from `MemberRemoved` and
/// `MemberLeft`. The field was added in v4 with the intent that B3's
/// descend-from check would consult it; in the actually-shipped B3 the
/// descend-from boundary comes from the **state delta's** governance
/// position, and `cut.governance_dag_heads` was redundant with the
/// signed `SignedNamespaceOp.parent_op_hashes` carrying the same data.
/// The `expected_group_state_hash` and `expected_context_state_hashes`
/// fields (the post-apply convergence claims that drive
/// reconcile-on-mismatch) remain. Pre-1.0 wire break; old-shape ops are
/// rejected outright.
///
/// v4: `MemberRemoved` and `MemberLeft` carry the signed governance-DAG
/// cut, the expected post-apply group state hash, and the expected
/// post-apply per-context CRDT state hashes. Receivers detect divergence by
/// comparing their computed hashes against the signed values after apply.
/// Pre-1.0 wire change; old-shape ops are rejected outright.
///
/// v3: Added `state_hash: [u8; 32]` — each op commits to the group's
/// authorization-relevant state at signing time. On apply, a non-zero
/// state hash is verified against the current state to reject stale ops.
///
/// v2: `parent_op_hash: Option` changed to `parent_op_hashes: Vec` for
/// multi-parent DAG support. See `DAG-BASED-GOVERNANCE.md`.
///
/// v1 was internal to feature branch development and never deployed to any
/// persistent network. No backward-compatible deserialization is needed.
///
/// Schema v6: adds `CascadeTargetApplicationSet` and
/// `CascadeGroupMigrationSet` variants at the END of the `GroupOp` enum
/// for namespace-level application-upgrade cascade. Variant ordinals for
/// every prior variant are preserved (Borsh tags variants by source
/// order). Older peers cannot deserialize these new variants -- the
/// rollout posture is operator-discipline: deploy v6 to every peer
/// before triggering a cascade. See
/// docs/superpowers/specs/2026-05-22-namespace-cascade-app-upgrade-design.md.
///
/// Schema v7: replaces the two ordered cascade ops
/// (`CascadeTargetApplicationSet` + `CascadeGroupMigrationSet`) with a single
/// atomic `CascadeUpgrade` op
/// that carries `cascade_hlc` — a fence timestamp stamped once by the
/// initiator so every node records an identical boundary. This eliminates
/// the out-of-order apply window that existed between the two v6 ops.
/// Variant ordinal is appended after the v6 variants; v6 ordinals are
/// preserved.
///
/// v8 (cutover C5.S3b): dropped the vestigial `state_hash` field from the
/// signable/​signed op. It stopped being an apply gate in C5.S3a (`scope_root`
/// is the authoritative convergence signal now), so it was pure dead weight in
/// the signed bytes. Removing it changes every op's content hash (the op id),
/// hence the version bump and the flag-day re-bootstrap.
pub const SIGNED_GROUP_OP_SCHEMA_VERSION: u8 = 8;

/// Domain separation prefix for Ed25519 signatures over group ops.
pub const GROUP_GOVERNANCE_SIGN_DOMAIN: &[u8] = b"calimero.group.v1";

/// A non-zero `u8` bitmask representing per-context member capabilities.
///
/// Validated at Borsh deserialization time: a zero value is rejected on the
/// wire, making it impossible to construct an invalid capability op from
/// received bytes.
///
/// Unknown bits are accepted without error (forward-compatible): nodes
/// running older software will store ops with bits they do not recognise and
/// replay them faithfully. Callers must mask against known capability
/// constants before interpreting the value.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, BorshSerialize)]
pub struct ContextCapabilityBits(u8);

impl ContextCapabilityBits {
    /// Construct from a raw bitmask, returning `None` if `bits == 0`.
    #[must_use]
    pub fn new(bits: u8) -> Option<Self> {
        if bits == 0 {
            None
        } else {
            Some(Self(bits))
        }
    }

    #[must_use]
    pub fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for ContextCapabilityBits {
    type Error = &'static str;

    fn try_from(bits: u8) -> Result<Self, Self::Error> {
        Self::new(bits).ok_or("capability bitmask must not be zero")
    }
}

impl From<std::num::NonZeroU8> for ContextCapabilityBits {
    fn from(v: std::num::NonZeroU8) -> Self {
        Self(v.get())
    }
}

impl BorshDeserialize for ContextCapabilityBits {
    fn deserialize_reader<R: io::Read>(reader: &mut R) -> io::Result<Self> {
        let bits = u8::deserialize_reader(reader)?;
        // Route through the canonical constructor so the zero-rejection
        // invariant lives in exactly one place.
        Self::new(bits).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "ContextCapabilityBits: capability bitmask must not be zero",
            )
        })
    }
}

/// Group mutation for local governance (signed, gossip-replicated).
///
/// Aligns with CLI / contract surfaces where feasible; see
/// `docs/context-management/LOCAL-GROUP-GOVERNANCE.md`.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
#[non_exhaustive]
pub enum GroupOp {
    /// Reserved for tests / padding.
    Noop,
    /// Add a member with a role.
    MemberAdded {
        member: PublicKey,
        role: GroupMemberRole,
    },
    /// Remove a member.
    ///
    /// Carries two signed claims that drive cross-DAG convergence on
    /// receive:
    ///
    /// - `expected_group_state_hash`: governance state hash
    ///   (`compute_group_state_hash`) AFTER the admin's local apply.
    ///   Receivers compute their own post-apply hash and compare;
    ///   mismatch signals divergence in membership rows / capabilities
    ///   / meta. Computed by simulating the removal pre-apply (pure
    ///   function of current members minus the removed identity).
    ///
    /// - `expected_context_state_hashes`: per-context CRDT root hash at
    ///   sign time, one entry per context registered under `group_id`,
    ///   sorted by `context_id` for deterministic op-content hashing.
    ///   Catches the case where a receiver applied legitimate
    ///   pre-removal state-DAG deltas from the now-removed member that
    ///   never reached the admin — the receiver's post-apply context
    ///   hash differs, divergence is detectable, anchor-sync reconcile fires.
    ///
    /// Apply-time mismatch is logged as a structured warning but does
    /// not roll back the apply; the canonical view is the admin's, and
    /// reconciliation against an anchor is the recovery path.
    ///
    /// **DAG ordering** is established by the enclosing signed envelope's
    /// `parent_op_hashes` (which receivers consult via the namespace
    /// governance DAG's `Pending` → backfill machinery). The B3 cross-DAG
    /// descend-from check uses the **state delta's** governance position,
    /// not a cut embedded here — every state delta carries its own
    /// `governance_position` against which `membership_status_at` walks.
    MemberRemoved {
        member: PublicKey,
        expected_group_state_hash: [u8; 32],
        expected_context_state_hashes: Vec<(ContextId, [u8; 32])>,
    },
    /// Self-leave: a member voluntarily exits the group. Distinct from
    /// `MemberRemoved` for audit clarity (this is a member choosing to
    /// leave; `MemberRemoved` is admin-initiated). Apply requires
    /// `signer == member`. Does NOT trigger the key-rotation pipeline —
    /// the leaver cannot generate the new key without retaining it, and
    /// proper forward secrecy requires a follow-up two-phase rotation
    /// (planned as a follow-up). For full cryptographic leave today,
    /// pair with admin-initiated `MemberRemoved`.
    /// See `architecture/membership-and-leave.html` § 5.
    ///
    /// Carries the same two convergence claims as `MemberRemoved`,
    /// signed by the leaver. The leaver is honest by definition for
    /// self-leave; a Byzantine leaver claiming false canonical hashes
    /// triggers a divergence warning on apply at each receiver but the
    /// op is **not rejected** — the apply path logs the mismatch and
    /// moves on, leaving the canonical view to be re-established by an
    /// admin-signed `MemberRemoved` or anchor-sync reconcile. The
    /// signed hashes are a detection signal, not an adoption gate.
    MemberLeft {
        member: PublicKey,
        expected_group_state_hash: [u8; 32],
        expected_context_state_hashes: Vec<(ContextId, [u8; 32])>,
    },
    /// Set a member’s role (same as upsert member with new role).
    MemberRoleSet {
        member: PublicKey,
        role: GroupMemberRole,
    },
    /// Per-member capability bitmask (`GroupMemberCapability` store).
    MemberCapabilitySet {
        member: PublicKey,
        capabilities: MemberCapabilities,
    },
    /// Default capability bitmask for new members.
    DefaultCapabilitiesSet { capabilities: MemberCapabilities },
    /// Update group upgrade policy in [`GroupMetaValue`].
    UpgradePolicySet { policy: UpgradePolicy },
    /// Update target application and app key in group metadata.
    TargetApplicationSet {
        app_key: AppKey,
        target_application_id: ApplicationId,
    },
    /// Register a context index under this group (must match `ContextGroupRef` invariants).
    ContextRegistered {
        context_id: ContextId,
        application_id: calimero_primitives::application::ApplicationId,
        blob_id: calimero_primitives::blobs::BlobId,
        /// Source URL for the application (registry URL or `file://` for dev).
        /// Joiners use this to install the app directly without blob sharing.
        source: String,
        /// Which service from the application bundle this context runs.
        /// None for single-service applications.
        service_name: Option<String>,
    },
    /// Unregister a context from this group.
    ContextDetached { context_id: ContextId },
    /// Subgroup visibility. When `Open`, parent-group members holding
    /// `CAN_JOIN_OPEN_SUBGROUPS` are inherited as members of this subgroup.
    /// See [`crate::group::SetSubgroupVisibilityRequest`].
    SubgroupVisibilitySet { mode: VisibilityMode },
    /// Wholly replace the metadata record (name + opaque `data`) of the group
    /// itself (a namespace is a root group, so this covers it).
    /// **Signer:** group admin or holder of `CAN_MANAGE_METADATA`.
    GroupMetadataSet {
        name: Option<String>,
        data: BTreeMap<String, String>,
    },
    /// Wholly replace a group member's metadata record.
    /// **Signer:** group admin, holder of `CAN_MANAGE_METADATA`, or the member
    /// themselves.
    MemberMetadataSet {
        member: PublicKey,
        name: Option<String>,
        data: BTreeMap<String, String>,
    },
    /// Wholly replace a group-registered context's metadata record.
    /// **Signer:** group admin or holder of `CAN_MANAGE_METADATA`.
    ContextMetadataSet {
        context_id: ContextId,
        name: Option<String>,
        data: BTreeMap<String, String>,
    },
    /// Delete the group locally (no registered contexts; same constraints as CLI delete).
    GroupDelete,
    /// Update group migration bytes in [`GroupMetaValue`] (admin).
    GroupMigrationSet { migration: Option<Vec<u8>> },
    /// Grant a capability to a member for a specific context.
    ContextCapabilityGranted {
        context_id: ContextId,
        member: PublicKey,
        capability: ContextCapabilityBits,
    },
    /// Revoke a capability from a member for a specific context.
    ContextCapabilityRevoked {
        context_id: ContextId,
        member: PublicKey,
        capability: ContextCapabilityBits,
    },
    /// TEE admission policy: defines which TEE nodes can auto-join the group.
    /// Only admins can set this policy.
    TeeAdmissionPolicySet {
        allowed_mrtd: Vec<String>,
        allowed_rtmr0: Vec<String>,
        allowed_rtmr1: Vec<String>,
        allowed_rtmr2: Vec<String>,
        allowed_rtmr3: Vec<String>,
        allowed_tcb_statuses: Vec<String>,
        accept_mock: bool,
    },
    /// A TEE node was admitted via attestation that matched the group's policy.
    /// Signed by an existing member who verified the attestation.
    MemberJoinedViaTeeAttestation {
        member: PublicKey,
        quote_hash: [u8; 32],
        mrtd: String,
        rtmr0: String,
        rtmr1: String,
        rtmr2: String,
        rtmr3: String,
        tcb_status: String,
        role: GroupMemberRole,
    },
    /// Set a member's auto-follow flags. When `auto_follow_contexts` is
    /// true, the auto-follow handler auto-joins new contexts registered
    /// in this group on behalf of `target`. When `auto_follow_subgroups`
    /// is true, the handler self-admits into subgroups nested under this
    /// group. Authorized by group admin (for any target) or by the target
    /// member themselves (self-setting). See the auto-follow architecture doc.
    MemberSetAutoFollow {
        target: PublicKey,
        auto_follow_contexts: bool,
        auto_follow_subgroups: bool,
    },
    /// Transfer ownership of this group to `new_owner`. Signer must be the
    /// current Owner; `new_owner` must already be a member. Updates
    /// `GroupMetaValue.owner_identity`. The previous owner remains a
    /// regular admin (no automatic role change beyond the owner field).
    /// See `architecture/membership-and-leave.html` § 7.
    TransferOwnership { new_owner: PublicKey },
    /// Cascade variant of [`Self::TargetApplicationSet`]: update the
    /// target application on the signed group AND on every descendant
    /// subgroup whose current `app_key` equals `from_app_key`. Walked at
    /// apply time against the receiver's local tree state, bounded by
    /// the namespace tree's maximum nesting depth (enforced in the
    /// apply handler in `calimero-context`). Same `manage_application`
    /// permission as the non-cascade variant on the signed group.
    /// Descendants that run a different application (different
    /// `app_key`) are skipped -- heterogeneous deployments stay untouched.
    ///
    /// `from_app_key` is the matching predicate, snapshotted at the
    /// emission peer's RPC-handling time. Carried in the op itself so
    /// every receiver applies the same filter against its own tree
    /// state -- ensures cross-peer convergence even when local stores
    /// have diverged between the emission peer's snapshot and a remote
    /// peer's apply time.
    ///
    /// Operationally invoked by an `upgrade_group` RPC with `cascade:
    /// true`. See `docs/superpowers/specs/2026-05-22-namespace-cascade-app-upgrade-design.md`.
    ///
    /// DEPRECATED: superseded by [`Self::CascadeUpgrade`], which applies the
    /// target + app_key + migration atomically in one op (no inter-op ordering
    /// window). Do NOT emit this; the apply arm is retained for one release so
    /// in-flight / replayed ops from pre-upgrade peers still apply. (Not marked
    /// `#[deprecated]` because the enum's derived borsh/Debug impls reference
    /// every variant and would warn at the derive site.)
    CascadeTargetApplicationSet {
        from_app_key: AppKey,
        app_key: AppKey,
        target_application_id: ApplicationId,
    },
    /// Cascade variant of [`Self::GroupMigrationSet`]: emitted alongside
    /// [`Self::CascadeTargetApplicationSet`] when the operator requested
    /// cascade-with-migration. Updates the migration bytes on the signed
    /// group AND on every descendant subgroup whose current `app_key`
    /// equals `from_app_key`. Same matching-predicate semantics as the
    /// target variant.
    ///
    /// DEPRECATED: superseded by [`Self::CascadeUpgrade`] (see that variant).
    /// Do NOT emit; apply arm retained for one release for wire-compat.
    CascadeGroupMigrationSet {
        from_app_key: AppKey,
        migration: Option<Vec<u8>>,
    },
    /// Atomic namespace cascade upgrade. Applies target_application_id, app_key,
    /// and migration in a SINGLE op per matched descendant (the
    /// `from_app_key == descendant.app_key` walk predicate), so receivers cannot
    /// reproduce the out-of-order apply bug. `cascade_hlc` is stamped once by the
    /// initiator so every node records an identical fence boundary. Lockstep
    /// wire addition (schema v7).
    ///
    /// `cascade_hlc` is the fence boundary read by the receive-path HLC fence
    /// (`calimero_context::hlc_fence`): any context-state delta whose HLC
    /// timestamp predates this value is rejected post-`Completed` to prevent
    /// offline-writer stale-schema state from overwriting migrated state.
    CascadeUpgrade {
        from_app_key: AppKey,
        app_key: AppKey,
        target_application_id: ApplicationId,
        migration: Option<Vec<u8>>,
        cascade_hlc: HybridTimestamp,
    },
}

impl GroupOp {
    /// Stable observability label for `op_kind` on
    /// `governance_publish_mesh_peers_at_publish` and other
    /// per-variant metrics. Defined in the same crate as `GroupOp` so
    /// the match is naturally exhaustive — adding a new variant
    /// surfaces as a compile error here, keeping the metric label set
    /// in sync with the wire format.
    #[must_use]
    pub fn op_kind_label(&self) -> &'static str {
        match self {
            GroupOp::Noop => "noop",
            GroupOp::MemberAdded { .. } => "member_added",
            GroupOp::MemberRemoved { .. } => "member_removed",
            GroupOp::MemberLeft { .. } => "member_left",
            GroupOp::MemberRoleSet { .. } => "member_role_set",
            GroupOp::MemberCapabilitySet { .. } => "member_capability_set",
            GroupOp::DefaultCapabilitiesSet { .. } => "default_capabilities_set",
            GroupOp::UpgradePolicySet { .. } => "upgrade_policy_set",
            GroupOp::TargetApplicationSet { .. } => "target_application_set",
            GroupOp::ContextRegistered { .. } => "context_registered",
            GroupOp::ContextDetached { .. } => "context_detached",
            GroupOp::SubgroupVisibilitySet { .. } => "subgroup_visibility_set",
            GroupOp::GroupMetadataSet { .. } => "group_metadata_set",
            GroupOp::MemberMetadataSet { .. } => "member_metadata_set",
            GroupOp::ContextMetadataSet { .. } => "context_metadata_set",
            GroupOp::GroupDelete => "group_delete",
            GroupOp::GroupMigrationSet { .. } => "group_migration_set",
            GroupOp::ContextCapabilityGranted { .. } => "context_capability_granted",
            GroupOp::ContextCapabilityRevoked { .. } => "context_capability_revoked",
            GroupOp::TransferOwnership { .. } => "transfer_ownership",
            GroupOp::TeeAdmissionPolicySet { .. } => "tee_admission_policy_set",
            GroupOp::MemberJoinedViaTeeAttestation { .. } => "member_joined_via_tee",
            GroupOp::MemberSetAutoFollow { .. } => "member_set_auto_follow",
            GroupOp::CascadeTargetApplicationSet { .. } => "cascade_target_application_set",
            GroupOp::CascadeGroupMigrationSet { .. } => "cascade_group_migration_set",
            GroupOp::CascadeUpgrade { .. } => "cascade_upgrade",
        }
    }
}

// ---------------------------------------------------------------------------
// Namespace-scoped governance ops (Phase 2 rewrite)
// ---------------------------------------------------------------------------

/// Top-level operation in the single namespace governance DAG.
///
/// Every delta in the DAG carries exactly one `NamespaceOp`:
/// - `Root` ops are cleartext and visible to all namespace members.
/// - `Group` ops have a cleartext `group_id` tag (for topic routing and
///   skeleton storage) but the actual mutation is encrypted.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
#[non_exhaustive]
pub enum NamespaceOp {
    /// Cleartext namespace-wide administrative operation.
    Root(RootOp),
    /// Encrypted group-scoped operation. The `group_id` and `key_id` are
    /// cleartext so non-members can store the skeleton; the payload is only
    /// readable by holders of the group key identified by `key_id`.
    Group {
        group_id: [u8; 32],
        /// `sha256(group_key)` — identifies which group key encrypted this op.
        key_id: KeyId,
        encrypted: EncryptedGroupOp,
        /// Present only on `MemberRemoved` ops: wraps a NEW group key for
        /// each remaining member. Lives outside the encrypted payload so
        /// the removed member cannot read it.
        key_rotation: Option<KeyRotation>,
    },
}

/// Cleartext administrative operations that affect the entire namespace.
// Intentionally NOT #[non_exhaustive]: `RootOp` is dispatched by a central
// exhaustive `match root` (governance op-apply) that must fail to compile when
// a variant is added so the new op gets a handler, rather than silently
// hitting a `_` arm and being dropped from application.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum RootOp {
    /// A new group was created AND atomically nested under `parent_id`.
    /// `parent_id` MUST reference a group that exists in this namespace
    /// (the namespace root itself or a previously-created subgroup).
    /// There is no orphan-creation path: every group is born nested.
    ///
    /// `restricted` sets the subgroup's visibility atomically at birth
    /// (#2771): `true` = Restricted (default, preserves legacy behavior),
    /// `false` = born-Open. A born-Open subgroup is Open at
    /// `SubgroupCreated`-event time, so `tee_subgroup_admit` skips it (TEE
    /// reads via inheritance) — eliminating the redundant transient direct
    /// `ReadOnlyTee` row that the old Restricted-then-flip path produced.
    /// Visibility can still be changed later via `SubgroupVisibilitySet`.
    GroupCreated {
        group_id: [u8; 32],
        parent_id: [u8; 32],
        restricted: bool,
    },
    /// Atomically move `child_group_id` from its current parent to
    /// `new_parent_id`. Both groups MUST exist in this namespace.
    /// Must not create a cycle (`new_parent_id` cannot be a descendant
    /// of `child_group_id`). `child_group_id` MUST NOT be the namespace
    /// root. Idempotent on `new_parent_id == old_parent_id`.
    ///
    /// Replaces the old `GroupNested` + `GroupUnnested` two-op pattern;
    /// orphan state is no longer expressible.
    GroupReparented {
        child_group_id: [u8; 32],
        new_parent_id: [u8; 32],
    },
    /// Delete `root_group_id` AND its entire subtree AND all contained
    /// contexts in one op. The signer pre-computes `cascade_group_ids`
    /// (descendants in children-first order) and `cascade_context_ids`
    /// (every context registered on root or any descendant). Every peer
    /// re-enumerates locally and rejects the op if the payload disagrees
    /// with their state — deterministic-application check that catches
    /// silent divergence.
    GroupDeleted {
        root_group_id: [u8; 32],
        cascade_group_ids: Vec<[u8; 32]>,
        cascade_context_ids: Vec<[u8; 32]>,
    },
    /// The namespace administrator was changed.
    AdminChanged { new_admin: PublicKey },
    /// Namespace-wide policy was updated (extensible).
    PolicyUpdated { policy_bytes: Vec<u8> },
    /// A member joined a group via an admin-signed invitation.
    ///
    /// **Cleartext** because the joiner doesn't hold the group key yet.
    /// The outer `SignedNamespaceOp` MUST be signed by the joining member
    /// (proves key ownership). Peers verify:
    ///
    /// 1. `signed_invitation.inviter_signature` is from a group admin
    /// 2. `signed_invitation.invitation.group_id` matches this op's context
    /// 3. `SignedNamespaceOp.signer` == `member` (can't add someone else)
    /// 4. The invitation hasn't expired
    ///
    /// The **role** is inside `signed_invitation.invitation.invited_role`
    /// (covered by admin's signature, joiner cannot escalate).
    ///
    /// After peers apply this, the joiner obtains the group key by
    /// requesting it directly from a sync peer that holds it (the
    /// pull-based key-delivery path), not from any op on this DAG.
    MemberJoined {
        member: PublicKey,
        /// The full admin-signed invitation — carries the inviter's
        /// identity, group_id, expiration, role, and the admin's
        /// signature. Peers use this to verify the join was authorized.
        signed_invitation: SignedGroupOpenInvitation,
    },
    /// Delivers a group key to a specific member, ECDH-wrapped so only
    /// the recipient can decrypt it.
    ///
    /// Published by an **admin-initiated** flow that adds a member to a
    /// group it can't yet decrypt — `add_group_members` (Restricted
    /// subgroup add) and TEE admission (`admit_tee_node`). The recipient
    /// picks it up off the DAG/gossip and decrypts its buffered ops.
    /// This is a **one-shot** publish per add (it is NOT re-published on
    /// backfill rounds — that receiver-side re-publish, the #2319 source,
    /// has been removed in favour of the joiner-side pull). The pull
    /// (`recover_missing_group_keys`) is the durable retry that covers a
    /// member who misses this delivery.
    KeyDelivery {
        group_id: [u8; 32],
        envelope: KeyEnvelope,
    },
    /// A namespace member just self-joined an `Open` subgroup whose
    /// `Open` visibility (+ their namespace-level
    /// `CAN_JOIN_OPEN_SUBGROUPS` capability) is the authorisation —
    /// there is no admin-signed invitation in this case.
    ///
    /// **Cleartext.** The outer `SignedNamespaceOp` MUST be signed by
    /// the joining member (proves key ownership). On apply, peers
    /// verify the joiner has an Inherited membership path to
    /// `group_id` via `check_group_membership_path`. The joiner then
    /// obtains the subgroup key via the pull-based key-delivery path
    /// (it requests the key directly from a key-holding sync peer when
    /// it next syncs the namespace and finds it lacks the key).
    ///
    /// This op closes the "self-join Open subgroup, can't decrypt
    /// state-DAG messages" gap — `join_context` previously inserted
    /// a `ContextIdentity` with `sender_key: None` for the inherited
    /// case and never asked any holder of the group key to deliver
    /// it. See `handlers/join_context.rs`.
    MemberJoinedOpen {
        member: PublicKey,
        group_id: [u8; 32],
    },
    /// Invitation-based join carrying the joiner's claimed redemption
    /// time (`joined_at`, unix seconds, covered by the joiner's
    /// signature). Apply rejects it when `joined_at` exceeds the
    /// invitation's `expiration_timestamp`, enforcing expiry
    /// deterministically on every node rather than only the joining one.
    MemberJoinedAt {
        member: PublicKey,
        signed_invitation: SignedGroupOpenInvitation,
        joined_at: u64,
    },
    /// **Namespace genesis (#2474).** The first op in every namespace DAG:
    /// authoritatively records the namespace's founding administrator/owner.
    ///
    /// Root-namespace creation previously emitted NO governance op — the
    /// founder lived only in the creator's local `GroupMetaValue`, so a
    /// bootstrapping replica replaying the synced DAG could never learn the
    /// owner and fell back to trust-on-first-use seeding from the
    /// KeyDelivery signer (`seed_bootstrap_admin_if_absent`). When the
    /// deliverer was a non-owner member the replica pinned the WRONG admin
    /// and permanently rejected the true owner's ops, wedging backfill.
    ///
    /// This op closes that gap: emitted as the genesis — the FIRST op in the
    /// namespace DAG, defined by having NO parents (its nonce is 1, since the
    /// head record defaults `next_nonce` to 1 when absent; `op.nonce` is
    /// informational/signature-covered, sequencing comes from
    /// `read_head_record().next_nonce`), `state_hash == [0u8; 32]` —
    /// signed with the namespace signing key, it
    /// is **self-authorizing** — apply does NOT call `require_namespace_admin`
    /// because genesis is what establishes that authority. It writes the root
    /// `GroupMetaValue` with `admin_identity == owner_identity == founder` and
    /// an Admin member row for the founder with default caps.
    ///
    /// **Anti-hijack:** apply is a no-op if the namespace already has root
    /// meta (an established founder). A forged second genesis cannot overwrite
    /// an existing admin; apply is idempotent.
    ///
    /// **Trust note (#2932):** this is the self-authorizing namespace genesis;
    /// founder authenticity on a BARE (not-yet-established) replica is
    /// trust-on-first-sync — the anti-hijack guarantee only protects an
    /// already-established namespace, not the first genesis on a bare one. See
    /// the SECURITY residual in `governance-store`'s `namespace_created.rs` and
    /// the #2932 root-of-trust follow-up.
    ///
    /// **Wire note:** appended at the END of `RootOp` so existing borsh
    /// discriminants do not renumber. It is still a borsh schema addition;
    /// consumers pinning this crate (e.g. mero-tee) must reset/coordinate a
    /// core-rev bump.
    NamespaceCreated { founder: PublicKey },
}

impl NamespaceOp {
    /// Stable observability label for `op_kind`. See
    /// [`GroupOp::op_kind_label`] for the rationale around defining this
    /// in the same crate as the enum: a new variant added here surfaces
    /// as a compile error so metric labels stay in sync with the wire
    /// format.
    #[must_use]
    pub fn op_kind_label(&self) -> &'static str {
        match self {
            NamespaceOp::Root(RootOp::GroupCreated { .. }) => "group_created",
            NamespaceOp::Root(RootOp::GroupReparented { .. }) => "group_reparented",
            NamespaceOp::Root(RootOp::GroupDeleted { .. }) => "group_deleted",
            NamespaceOp::Root(RootOp::AdminChanged { .. }) => "admin_changed",
            NamespaceOp::Root(RootOp::PolicyUpdated { .. }) => "policy_updated",
            NamespaceOp::Root(RootOp::MemberJoined { .. }) => "member_joined",
            NamespaceOp::Root(RootOp::MemberJoinedAt { .. }) => "member_joined_at",
            NamespaceOp::Root(RootOp::MemberJoinedOpen { .. }) => "member_joined_open",
            NamespaceOp::Root(RootOp::KeyDelivery { .. }) => "key_delivery",
            NamespaceOp::Root(RootOp::NamespaceCreated { .. }) => "namespace_created",
            NamespaceOp::Group { .. } => "group_op",
        }
    }
}

/// An encrypted group operation payload. Only members of the group
/// (who possess the group key) can decrypt the inner [`GroupOp`].
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct EncryptedGroupOp {
    /// 12-byte AES-GCM nonce.
    pub nonce: [u8; 12],
    /// `AES-256-GCM(borsh(GroupOp))` using the group key.
    pub ciphertext: Vec<u8>,
}

/// ECDH-wrapped group key for a specific recipient.
///
/// The sender encrypts the group key using a shared secret derived from
/// `SharedKey::new(sender_sk, recipient_pk)`. The recipient decrypts with
/// `SharedKey::new(recipient_sk, ephemeral_pk)`.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct KeyEnvelope {
    /// Recipient's namespace identity public key.
    pub recipient: PublicKey,
    /// Sender's public key used for ECDH key agreement.
    pub ephemeral_pk: PublicKey,
    /// 12-byte AES-GCM nonce.
    pub nonce: [u8; 12],
    /// `AES-256-GCM(group_key)` using the ECDH shared secret.
    pub ciphertext: Vec<u8>,
}

/// Key rotation bundle attached to a `MemberRemoved` governance op.
///
/// Contains the new key's identifier and ECDH-wrapped envelopes for every
/// remaining group member. The removed member receives no envelope and is
/// cryptographically locked out of all future data.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct KeyRotation {
    /// `sha256(new_group_key)` — identifies the new epoch.
    pub new_key_id: KeyId,
    /// One envelope per remaining member, each wrapping the new group key.
    pub envelopes: Vec<KeyEnvelope>,
}

/// Signable envelope for a namespace governance operation.
///
/// Mirrors [`SignableGroupOp`] but wraps [`NamespaceOp`] instead of
/// [`GroupOp`], and is scoped to a namespace rather than a single group.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SignableNamespaceOp {
    pub version: u8,
    pub namespace_id: NamespaceId,
    pub parent_op_hashes: Vec<[u8; 32]>,
    pub signer: PublicKey,
    pub nonce: u64,
    pub op: NamespaceOp,
}

/// A signed namespace governance operation ready for gossip or storage.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SignedNamespaceOp {
    pub version: u8,
    pub namespace_id: NamespaceId,
    pub parent_op_hashes: Vec<[u8; 32]>,
    pub signer: PublicKey,
    pub nonce: u64,
    pub op: NamespaceOp,
    pub signature: [u8; 64],
}

/// Wire/schema version for [`SignedNamespaceOp`].
///
/// v2 (cutover C5.S3b): dropped the vestigial `state_hash` field. It stopped
/// being an apply gate in C5.S3a (`scope_root` is the convergence signal now);
/// removing it from the signable/​signed structs changes every op id, hence the
/// version bump and the flag-day re-bootstrap.
pub const SIGNED_NAMESPACE_OP_SCHEMA_VERSION: u8 = 2;

/// Domain separation prefix for Ed25519 signatures over namespace ops.
pub const NAMESPACE_GOVERNANCE_SIGN_DOMAIN: &[u8] = b"calimero.namespace.v1";

/// Bytes that are hashed/signed for a namespace op.
pub fn namespace_signable_bytes(
    signable: &SignableNamespaceOp,
) -> Result<Vec<u8>, GovernanceError> {
    let mut body = borsh::to_vec(signable)?;
    let mut out = Vec::with_capacity(NAMESPACE_GOVERNANCE_SIGN_DOMAIN.len() + body.len());
    out.extend_from_slice(NAMESPACE_GOVERNANCE_SIGN_DOMAIN);
    out.append(&mut body);
    Ok(out)
}

/// Content hash for a namespace op (SHA-256 of [`namespace_signable_bytes`]).
pub fn namespace_op_content_hash(
    signable: &SignableNamespaceOp,
) -> Result<[u8; 32], GovernanceError> {
    let bytes = namespace_signable_bytes(signable)?;
    Ok(Sha256::digest(&bytes).into())
}

impl SignedNamespaceOp {
    /// Build and sign a new namespace operation.
    pub fn sign(
        sk: &PrivateKey,
        namespace_id: NamespaceId,
        parent_op_hashes: Vec<[u8; 32]>,
        nonce: u64,
        op: NamespaceOp,
    ) -> Result<Self, GovernanceError> {
        let signer = sk.public_key();
        let signable = SignableNamespaceOp {
            version: SIGNED_NAMESPACE_OP_SCHEMA_VERSION,
            namespace_id,
            parent_op_hashes,
            signer,
            nonce,
            op,
        };
        let msg = namespace_signable_bytes(&signable)?;
        let sig = sk.sign(&msg)?;
        Ok(Self {
            version: signable.version,
            namespace_id: signable.namespace_id,
            parent_op_hashes: signable.parent_op_hashes,
            signer: signable.signer,
            nonce: signable.nonce,
            op: signable.op,
            signature: sig.to_bytes(),
        })
    }

    /// Verify schema version and Ed25519 signature.
    ///
    /// **Cryptographic integrity only.** A successful return means
    /// `self.signature` is a valid signature over the canonical bytes
    /// produced from `self.signer`'s public key — nothing more. It
    /// does **not** verify that `self.signer` is an authorized
    /// member, admin, or current namespace participant. Callers MUST
    /// check membership/role authorization separately before
    /// accepting the op (the apply path in `calimero-context` does
    /// this via `membership_status_at` / `is_group_admin` /
    /// `is_authoritative_namespace_identity`). Treating this method
    /// as an authorization check would accept ops signed by revoked
    /// or never-admitted keys.
    pub fn verify_signature(&self) -> Result<(), GovernanceError> {
        if self.version != SIGNED_NAMESPACE_OP_SCHEMA_VERSION {
            return Err(GovernanceError::SchemaVersion {
                expected: SIGNED_NAMESPACE_OP_SCHEMA_VERSION,
                got: self.version,
            });
        }
        let signable = self.to_signable();
        let msg = namespace_signable_bytes(&signable)?;
        self.signer.verify_raw_signature(&msg, &self.signature)?;
        Ok(())
    }

    #[must_use]
    pub fn to_signable(&self) -> SignableNamespaceOp {
        SignableNamespaceOp {
            version: self.version,
            namespace_id: self.namespace_id,
            parent_op_hashes: self.parent_op_hashes.clone(),
            signer: self.signer,
            nonce: self.nonce,
            op: self.op.clone(),
        }
    }

    /// Content hash for this op's signable payload (for dedup / parent links).
    pub fn content_hash(&self) -> Result<[u8; 32], GovernanceError> {
        namespace_op_content_hash(&self.to_signable())
    }

    /// Extract the group_id if this is a group-scoped op.
    #[must_use]
    pub fn group_id(&self) -> Option<[u8; 32]> {
        match &self.op {
            NamespaceOp::Group { group_id, .. } => Some(*group_id),
            NamespaceOp::Root(_) => None,
        }
    }
}

/// An opaque skeleton stored by non-members for group-scoped ops they
/// cannot decrypt. Retains causal structure so the DAG remains valid.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct OpaqueSkeleton {
    pub delta_id: [u8; 32],
    pub parent_op_hashes: Vec<[u8; 32]>,
    pub group_id: [u8; 32],
    pub signer: PublicKey,
}

/// Tagged storage representation for namespace governance op-log rows.
///
/// This removes ambiguity from polymorphic storage payloads by explicitly
/// tagging whether a row contains a full signed op or an opaque skeleton.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
#[allow(
    clippy::large_enum_variant,
    reason = "gossip/stored message: constructed, borsh-serialized, then dropped; boxing the common large variant adds a per-message heap allocation without real benefit"
)]
pub enum StoredNamespaceEntry {
    Signed(SignedNamespaceOp),
    Opaque(OpaqueSkeleton),
}

// ---------------------------------------------------------------------------
// Original group-scoped types (kept for backward compat and as inner payload)
// ---------------------------------------------------------------------------

/// Payload that is actually signed (everything except the signature bytes).
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SignableGroupOp {
    pub version: u8,
    pub group_id: [u8; 32],
    pub parent_op_hashes: Vec<[u8; 32]>,
    pub signer: PublicKey,
    pub nonce: u64,
    pub op: GroupOp,
}

/// A signed group operation ready for gossip or storage.
///
/// Embeds DAG parent references for causal ordering: `parent_op_hashes`
/// contains the content hashes of all current DAG heads at signing time.
/// Single parent = linear chain; multiple parents = merge after concurrent ops.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SignedGroupOp {
    pub version: u8,
    pub group_id: [u8; 32],
    pub parent_op_hashes: Vec<[u8; 32]>,
    pub signer: PublicKey,
    pub nonce: u64,
    pub op: GroupOp,
    pub signature: [u8; 64],
}

#[derive(Debug, Error)]
pub enum GovernanceError {
    #[error("schema version mismatch: expected {expected}, got {got}")]
    SchemaVersion { expected: u8, got: u8 },
    #[error("signature verification failed: {0}")]
    Signature(#[from] SignatureError),
    #[error("borsh serialization failed: {0}")]
    BorshSerialize(#[from] std::io::Error),
}

/// Bytes that are hashed/signed: `GROUP_GOVERNANCE_SIGN_DOMAIN` || `borsh(SignableGroupOp)`.
pub fn signable_bytes(signable: &SignableGroupOp) -> Result<Vec<u8>, GovernanceError> {
    let mut body = borsh::to_vec(signable)?;
    let mut out = Vec::with_capacity(GROUP_GOVERNANCE_SIGN_DOMAIN.len() + body.len());
    out.extend_from_slice(GROUP_GOVERNANCE_SIGN_DOMAIN);
    out.append(&mut body);
    Ok(out)
}

/// Stable content id for idempotency: SHA-256 of [`signable_bytes`].
pub fn op_content_hash(signable: &SignableGroupOp) -> Result<[u8; 32], GovernanceError> {
    let bytes = signable_bytes(signable)?;
    Ok(Sha256::digest(&bytes).into())
}

impl SignedGroupOp {
    /// Build and sign a new operation with [`SIGNED_GROUP_OP_SCHEMA_VERSION`].
    ///
    /// `parent_op_hashes` should be the current DAG heads (content hashes of the
    /// latest applied ops). Empty vec for the first op in a group (genesis).
    pub fn sign(
        sk: &PrivateKey,
        group_id: [u8; 32],
        parent_op_hashes: Vec<[u8; 32]>,
        nonce: u64,
        op: GroupOp,
    ) -> Result<Self, GovernanceError> {
        let signer = sk.public_key();
        let signable = SignableGroupOp {
            version: SIGNED_GROUP_OP_SCHEMA_VERSION,
            group_id,
            parent_op_hashes,
            signer,
            nonce,
            op,
        };
        let msg = signable_bytes(&signable)?;
        let sig = sk.sign(&msg)?;
        Ok(Self {
            version: signable.version,
            group_id: signable.group_id,
            parent_op_hashes: signable.parent_op_hashes,
            signer: signable.signer,
            nonce: signable.nonce,
            op: signable.op,
            signature: sig.to_bytes(),
        })
    }

    /// Verify schema version and Ed25519 signature.
    ///
    /// **Cryptographic integrity only.** A successful return means
    /// `self.signature` is a valid signature over the canonical bytes
    /// produced from `self.signer`'s public key — nothing more. It
    /// does **not** verify that `self.signer` is an authorized
    /// member, admin, or current group participant. Callers MUST
    /// check membership/role authorization separately before
    /// accepting the op (the apply path in `calimero-context` does
    /// this via `membership_status_at`, `is_group_admin`, and the
    /// per-op permission checks).
    pub fn verify_signature(&self) -> Result<(), GovernanceError> {
        if self.version != SIGNED_GROUP_OP_SCHEMA_VERSION {
            return Err(GovernanceError::SchemaVersion {
                expected: SIGNED_GROUP_OP_SCHEMA_VERSION,
                got: self.version,
            });
        }
        let signable = self.to_signable();
        let msg = signable_bytes(&signable)?;
        self.signer.verify_raw_signature(&msg, &self.signature)?;
        Ok(())
    }

    #[must_use]
    pub fn to_signable(&self) -> SignableGroupOp {
        SignableGroupOp {
            version: self.version,
            group_id: self.group_id,
            parent_op_hashes: self.parent_op_hashes.clone(),
            signer: self.signer,
            nonce: self.nonce,
            op: self.op.clone(),
        }
    }

    /// Content hash for this op’s signable payload (for dedup / parent links).
    pub fn content_hash(&self) -> Result<[u8; 32], GovernanceError> {
        op_content_hash(&self.to_signable())
    }
}

#[cfg(test)]
mod tests;
