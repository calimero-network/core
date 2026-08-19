//! The change an op carries — every plane, one enum.
//!
//! This file is the crate's wire format. Its variant *order* is a persisted
//! consensus artifact rather than a stylistic choice, which is the reason
//! [`OpPayload`] lives alone: appending a variant should be a one-file diff
//! that touches no hashing or envelope code, and reordering one should look as
//! consequential as it is.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};

use calimero_account::{AccountGenesis, AccountId, DeviceCert, DeviceId, RootKeyHandoff};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::GroupMemberRole;
use calimero_storage::address::Id;
use calimero_storage::entities::OpMask;

use crate::scope::ScopeId;

/// The change an [`Op`](crate::Op) carries, across all four planes folded into
/// one model.
///
/// **Append-only wire format.** An op's content-address [`id`](crate::Op::id)
/// is a hash over `borsh(payload)`, and borsh encodes an enum variant by its
/// *positional* discriminant (declaration order → tag byte). Inserting a
/// variant in the middle, removing one, or reordering therefore renumbers every
/// later variant, which silently changes the id — and thus the signature — of
/// every already stored/persisted op that used one of the shifted variants. New
/// variants MUST be appended at the end only; existing variants must never be
/// reordered or removed. [`op_payload_discriminants_are_pinned`] guards this.
///
/// This enum is intentionally *not* `#[non_exhaustive]`: `calimero-authz`
/// authorizes ops with an exhaustive match over `OpPayload`, so a newly added
/// variant should fail to compile there until it is explicitly given an
/// authorization rule, rather than being swept into a catch-all arm.
///
/// [`op_payload_discriminants_are_pinned`]: crate::tests::payload::op_payload_discriminants_are_pinned
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub enum OpPayload {
    // ---- data plane ----
    /// Write `value` to `entity`.
    Put { entity: Id, value: Vec<u8> },
    /// Delete `entity`.
    Delete { entity: Id },

    // ---- access-control plane ----
    /// Set the writer/capability set for `object` (writer-set rotation).
    SetWriters {
        object: Id,
        writers: BTreeMap<AccountId, OpMask>,
    },

    // ---- membership plane ----
    /// Add `member` to `group` with `role`.
    MemberAdded {
        group: ContextGroupId,
        member: AccountId,
        role: GroupMemberRole,
    },
    /// Remove `member` from `group`.
    MemberRemoved {
        group: ContextGroupId,
        member: AccountId,
    },

    // ---- admin / namespace plane ----
    /// Change the scope's root admin.
    AdminChanged { new_admin: AccountId },
    /// Replace the scope's policy bytes.
    PolicyUpdated { policy_bytes: Vec<u8> },
    /// Create a child subgroup scope nested under `parent`. A `restricted`
    /// subgroup's very existence is hidden from non-members. `admin` is the
    /// creator — the subgroup's genesis admin (mirrors the live
    /// `GroupMeta.admin_identity = GroupCreated.signer`), so admin authority is
    /// resolvable from the projection without a separate membership op.
    SubgroupCreated {
        child: ScopeId,
        parent: ScopeId,
        restricted: bool,
        admin: AccountId,
    },
    /// Move a subgroup scope under a new parent (a scope-tree restructure).
    SubgroupReparented { child: ScopeId, new_parent: ScopeId },
    /// Delete a subgroup scope. Deleting a subtree is expressed as one
    /// `SubgroupDeleted` per cascaded scope.
    SubgroupDeleted { scope: ScopeId },
    /// Set a subgroup's visibility post-creation. `restricted == false` means
    /// Open (members of an open subgroup's open ancestor chain inherit
    /// membership); `true` means Restricted (a visibility wall). Mirrors the
    /// live `SubgroupVisibilitySet` op.
    SubgroupVisibilitySet { scope: ScopeId, restricted: bool },

    // ---- capability plane (drives inherited-membership resolution) ----
    /// Set `group`'s default member-capability bitmask (applied to members
    /// without an explicit override). The `CAN_JOIN_OPEN_SUBGROUPS` bit gates
    /// inheritance into open subgroups.
    DefaultCapabilitiesSet {
        group: ContextGroupId,
        capabilities: MemberCapabilities,
    },
    /// Set `member`'s explicit capability bitmask in `group` (overrides the
    /// group default for that member).
    MemberCapabilitySet {
        group: ContextGroupId,
        member: AccountId,
        capabilities: MemberCapabilities,
    },

    // ---- graph-only ----
    /// A node that changes no projection state but occupies its place in the
    /// causal graph. Used when a source-DAG op must be present so an ancestry
    /// walk can traverse *through* it to reach the ops behind it, yet the op
    /// itself carries nothing the projection models (e.g. a non-membership
    /// governance op, or an encrypted op this node can't decrypt). Folding it
    /// is a no-op; its only effect is keeping the parent chain unbroken.
    Noop,

    // ---- account plane ----
    //
    // Appended after `Noop` rather than grouped with the other governance
    // planes above: borsh tags by declaration order, so slotting a variant into
    // its thematic home would renumber every variant after it and silently
    // change the id — and therefore the signature — of every stored op that
    // used one. New variants go at the end, always.
    /// Bind a device to an account, within this scope.
    ///
    /// Self-contained by construction: `genesis` hashes to `cert.account`, and
    /// `chain` carries the signed root-key rollovers from the genesis up to
    /// `cert.key_epoch`. A verifier needs no prior op to check the credential —
    /// which is what lets a freshly paired device link *itself* into every
    /// scope its account belongs to, instead of the account root having to
    /// author a grant into each one.
    ///
    /// The op is authored by the new device itself and requires no admin
    /// action, because linking a device to an account that is **already a
    /// member** is not a privilege escalation: the account holds every right
    /// the device gains. The projection enforces exactly that (see its
    /// `DeviceLinked` fold rules), so a device can never link itself into a
    /// scope its account does not belong to.
    DeviceLinked {
        /// The account's self-certifying root; `genesis.account_id()` must
        /// equal `cert.account`.
        genesis: AccountGenesis,
        /// Signed root-key rollovers, epoch 0 upward, reaching
        /// `cert.key_epoch`. Empty when the cert was signed by the genesis key.
        chain: Vec<RootKeyHandoff>,
        /// The root-signed grant being folded.
        cert: DeviceCert,
    },
    /// Withdraw a device from an account, at this cut.
    ///
    /// Terminal for this `DeviceId`: re-enrolling the physical machine mints a
    /// fresh id. Making revocation permanent rather than a toggle means a
    /// replica id is never reused, so the CRDT planes keep their one-writer-per-
    /// replica invariant even across a revoke/re-add cycle.
    ///
    /// Causally honoured like every other decision — ops the device authored
    /// *before* this one in causal order stay valid, and ops after it do not.
    DeviceRevoked {
        /// Account the device is being removed from.
        account: AccountId,
        /// The device losing its binding.
        device: DeviceId,
    },
    /// Roll an account's root key within this scope.
    ///
    /// Folding this raises the account's key epoch, after which a certificate
    /// signed by any superseded key is refused — which is how a rotation
    /// actually withdraws the old key's authority rather than merely adding a
    /// new one alongside it.
    AccountKeysRotated {
        /// The handoff, signed by the outgoing key.
        handoff: RootKeyHandoff,
    },
    /// Add `member` to `group` **and** bind the device it joined with, as one
    /// indivisible fact.
    ///
    /// A join carries both halves, so folding them as two ops would admit an
    /// ordering in which the member is known and the device is not. That gap is
    /// not cosmetic: a node's writer principal is its bound account when a binding
    /// exists and a key-derived stand-in when it does not, so a member whose
    /// device has not folded yet is a member whose own writes resolve to a
    /// different principal than the one it writes under. One payload removes the
    /// ordering entirely.
    ///
    /// The membership half folds exactly as [`Self::MemberAdded`] and the device
    /// half exactly as [`Self::DeviceLinked`] — same LWW slot, same op-local
    /// credential rules — so this variant adds no new semantics, only atomicity.
    MemberJoinedWithDevice {
        /// Group being joined.
        group: ContextGroupId,
        /// The joining member.
        member: AccountId,
        /// Role granted by the invitation.
        role: GroupMemberRole,
        /// The joiner's self-certifying account root.
        genesis: AccountGenesis,
        /// Signed root-key rollovers reaching `cert.key_epoch`.
        chain: Vec<RootKeyHandoff>,
        /// The root-signed grant binding the joining device.
        cert: DeviceCert,
    },
}
