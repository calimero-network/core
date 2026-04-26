//! Signed group operations for **local** governance (no chain).
//!
//! See `docs/context-management/LOCAL-GROUP-GOVERNANCE.md`.
//!
//! ## Namespace governance model
//!
//! A **namespace** has a single governance DAG. Operations in the DAG are
//! either *root ops* (cleartext, visible to all namespace members) or
//! *group-scoped ops* (tagged with a cleartext `group_id` for routing, but
//! the payload is encrypted with the group's sender key so only group
//! members can read it). Non-members store an opaque **skeleton** for
//! group-scoped ops they cannot decrypt.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_context_config::types::SignedGroupOpenInvitation;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use ed25519_dalek::SignatureError;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Wire/schema version for [`SignedGroupOp`].
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
pub const SIGNED_GROUP_OP_SCHEMA_VERSION: u8 = 3;

/// Domain separation prefix for Ed25519 signatures over group ops.
pub const GROUP_GOVERNANCE_SIGN_DOMAIN: &[u8] = b"calimero.group.v1";

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
    MemberRemoved { member: PublicKey },
    /// Set a member’s role (same as upsert member with new role).
    MemberRoleSet {
        member: PublicKey,
        role: GroupMemberRole,
    },
    /// Per-member capability bitmask (`GroupMemberCapability` store).
    MemberCapabilitySet {
        member: PublicKey,
        capabilities: u32,
    },
    /// Default capability bitmask for new members.
    DefaultCapabilitiesSet { capabilities: u32 },
    /// Update group upgrade policy in [`GroupMetaValue`].
    UpgradePolicySet { policy: UpgradePolicy },
    /// Update target application and app key in group metadata.
    TargetApplicationSet {
        app_key: [u8; 32],
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
    /// Subgroup visibility (`0` = Open, `1` = Restricted). When `Open`,
    /// parent-group members holding `CAN_JOIN_OPEN_SUBGROUPS` are inherited
    /// as members of this subgroup. See [`crate::group::SetSubgroupVisibilityRequest`].
    SubgroupVisibilitySet { mode: u8 },
    /// Human-readable alias for a context within the group.
    /// **Signer:** group admin.
    ContextAliasSet {
        context_id: ContextId,
        alias: String,
    },
    /// Human-readable alias for a member within the group.
    MemberAliasSet { member: PublicKey, alias: String },
    /// Human-readable alias for the group itself.
    GroupAliasSet { alias: String },
    /// Delete the group locally (no registered contexts; same constraints as CLI delete).
    GroupDelete,
    /// Update group migration bytes in [`GroupMetaValue`] (admin).
    GroupMigrationSet { migration: Option<Vec<u8>> },
    /// Grant a capability to a member for a specific context.
    ContextCapabilityGranted {
        context_id: ContextId,
        member: PublicKey,
        capability: u8,
    },
    /// Revoke a capability from a member for a specific context.
    ContextCapabilityRevoked {
        context_id: ContextId,
        member: PublicKey,
        capability: u8,
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
}

// ---------------------------------------------------------------------------
// Namespace-scoped governance ops (Phase 2 rewrite)
// ---------------------------------------------------------------------------

/// Top-level operation in the single namespace governance DAG.
///
/// Every delta in the DAG carries exactly one `NamespaceOp`:
/// - `Root` ops are cleartext and visible to all namespace members.
/// - `Group` ops have a cleartext `group_id` tag (for topic routing and
///    skeleton storage) but the actual mutation is encrypted.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum NamespaceOp {
    /// Cleartext namespace-wide administrative operation.
    Root(RootOp),
    /// Encrypted group-scoped operation. The `group_id` and `key_id` are
    /// cleartext so non-members can store the skeleton; the payload is only
    /// readable by holders of the group key identified by `key_id`.
    Group {
        group_id: [u8; 32],
        /// `sha256(group_key)` — identifies which group key encrypted this op.
        key_id: [u8; 32],
        encrypted: EncryptedGroupOp,
        /// Present only on `MemberRemoved` ops: wraps a NEW group key for
        /// each remaining member. Lives outside the encrypted payload so
        /// the removed member cannot read it.
        key_rotation: Option<KeyRotation>,
    },
}

/// Cleartext administrative operations that affect the entire namespace.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum RootOp {
    /// A new group was created AND atomically nested under `parent_id`.
    /// `parent_id` MUST reference a group that exists in this namespace
    /// (the namespace root itself or a previously-created subgroup).
    /// There is no orphan-creation path: every group is born nested.
    GroupCreated {
        group_id: [u8; 32],
        parent_id: [u8; 32],
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
    /// After peers apply this, any existing member who holds the group key
    /// publishes a [`KeyDelivery`](RootOp::KeyDelivery) wrapping the key
    /// for the joiner via ECDH.
    MemberJoined {
        member: PublicKey,
        /// The full admin-signed invitation — carries the inviter's
        /// identity, group_id, expiration, role, and the admin's
        /// signature. Peers use this to verify the join was authorized.
        signed_invitation: SignedGroupOpenInvitation,
    },
    /// Delivers the current group key to a specific member.
    ///
    /// Published by an existing member after seeing `MemberJoined` on the
    /// DAG. The group key is ECDH-wrapped so only the recipient can
    /// decrypt it. No P2P handshake or online requirement — the joiner
    /// picks this up when it processes the DAG.
    KeyDelivery {
        group_id: [u8; 32],
        envelope: KeyEnvelope,
    },
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
    pub new_key_id: [u8; 32],
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
    pub namespace_id: [u8; 32],
    pub parent_op_hashes: Vec<[u8; 32]>,
    pub state_hash: [u8; 32],
    pub signer: PublicKey,
    pub nonce: u64,
    pub op: NamespaceOp,
}

/// A signed namespace governance operation ready for gossip or storage.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SignedNamespaceOp {
    pub version: u8,
    pub namespace_id: [u8; 32],
    pub parent_op_hashes: Vec<[u8; 32]>,
    pub state_hash: [u8; 32],
    pub signer: PublicKey,
    pub nonce: u64,
    pub op: NamespaceOp,
    pub signature: [u8; 64],
}

/// Wire/schema version for [`SignedNamespaceOp`].
pub const SIGNED_NAMESPACE_OP_SCHEMA_VERSION: u8 = 1;

/// Domain separation prefix for Ed25519 signatures over namespace ops.
pub const NAMESPACE_GOVERNANCE_SIGN_DOMAIN: &[u8] = b"calimero.namespace.v1";

/// Bytes that are hashed/signed for a namespace op.
pub fn namespace_signable_bytes(
    signable: &SignableNamespaceOp,
) -> Result<Vec<u8>, GovernanceError> {
    let mut body =
        borsh::to_vec(signable).map_err(|e| GovernanceError::BorshSerialize(e.to_string()))?;
    let mut out = Vec::with_capacity(NAMESPACE_GOVERNANCE_SIGN_DOMAIN.len() + body.len());
    out.extend_from_slice(NAMESPACE_GOVERNANCE_SIGN_DOMAIN);
    out.append(&mut body);
    Ok(out)
}

/// Content hash for a namespace op (SHA-256 of [`namespace_signable_bytes`]).
#[must_use]
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
        namespace_id: [u8; 32],
        parent_op_hashes: Vec<[u8; 32]>,
        state_hash: [u8; 32],
        nonce: u64,
        op: NamespaceOp,
    ) -> Result<Self, GovernanceError> {
        let signer = sk.public_key();
        let signable = SignableNamespaceOp {
            version: SIGNED_NAMESPACE_OP_SCHEMA_VERSION,
            namespace_id,
            parent_op_hashes,
            state_hash,
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
            state_hash: signable.state_hash,
            signer: signable.signer,
            nonce: signable.nonce,
            op: signable.op,
            signature: sig.to_bytes(),
        })
    }

    /// Verify schema version and Ed25519 signature.
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
            state_hash: self.state_hash,
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
    pub state_hash: [u8; 32],
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
    pub state_hash: [u8; 32],
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
    BorshSerialize(String),
}

/// Bytes that are hashed/signed: `GROUP_GOVERNANCE_SIGN_DOMAIN` || `borsh(SignableGroupOp)`.
pub fn signable_bytes(signable: &SignableGroupOp) -> Result<Vec<u8>, GovernanceError> {
    let mut body =
        borsh::to_vec(signable).map_err(|e| GovernanceError::BorshSerialize(e.to_string()))?;
    let mut out = Vec::with_capacity(GROUP_GOVERNANCE_SIGN_DOMAIN.len() + body.len());
    out.extend_from_slice(GROUP_GOVERNANCE_SIGN_DOMAIN);
    out.append(&mut body);
    Ok(out)
}

/// Stable content id for idempotency: SHA-256 of [`signable_bytes`].
#[must_use]
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
        state_hash: [u8; 32],
        nonce: u64,
        op: GroupOp,
    ) -> Result<Self, GovernanceError> {
        let signer = sk.public_key();
        let signable = SignableGroupOp {
            version: SIGNED_GROUP_OP_SCHEMA_VERSION,
            group_id,
            parent_op_hashes,
            state_hash,
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
            state_hash: signable.state_hash,
            signer: signable.signer,
            nonce: signable.nonce,
            op: signable.op,
            signature: sig.to_bytes(),
        })
    }

    /// Verify schema version and Ed25519 signature.
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
            state_hash: self.state_hash,
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
