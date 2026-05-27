//! Typed error enums for `group_store` domain operations (#2305).
//!
//! Each enum represents a single domain (membership, namespace topology,
//! capability/permission, encryption keys, etc.). Repository methods bail
//! with the typed variant via `bail!(DomainError::Variant)`; the variant
//! flows through `eyre::Report` and can be recovered by callers with
//! `err.downcast_ref::<DomainError>()`.
//!
//! # Downcast asymmetry
//!
//! `bail!(DomainError::Variant)` stores the **`DomainError` type**
//! inside `eyre::Report` — callers must `downcast_ref` to the same
//! type that was bailed:
//!
//! ```ignore
//! // sites that bail!(MembershipError::LastAdmin)
//! err.downcast_ref::<MembershipError>()
//!
//! // sites that bail!(ApplyError::StateHashMismatch { .. })
//! err.downcast_ref::<ApplyError>()
//! ```
//!
//! The `#[from]` impls on `ApplyError` are NOT triggered by `?` here
//! (every method returns `EyreResult`, not `Result<_, ApplyError>`)
//! — they exist for explicit `.into()` conversions when apply-path
//! code wants to wrap a domain error as an `ApplyError` for logging
//! / typed return.
//!
//! In short: downcast to the type that was bailed, not to
//! `ApplyError`. The composition exists for the future state where
//! method signatures move to `Result<_, ApplyError>` (#2304 / #2481);
//! today it documents the type relationship without forcing a
//! signature migration.
//!
//! **Why per-domain instead of one giant enum?** Callers (server handlers,
//! tests, governance apply) need to discriminate by *meaning*, not by
//! source file. Splitting along Repository lines would conflate truly
//! distinct error semantics (e.g. `LastAdmin` and `NestingCycle` are not
//! the same kind of failure even though both could come out of
//! `MembershipRepository` via `recursive_remove_member`).
//!
//! **Why keep `EyreResult<T>` as the method signature?** Threading typed
//! errors through every method signature would cascade into ~200 call
//! sites for a refactor whose payoff is callers that *care* about
//! specific variants. Those callers use `downcast_ref` (zero-cost when
//! the variant matches); callers that just bubble errors stay unchanged.
//! See #2305 issue discussion.
//!
//! **Composition.** `ApplyError` is the top-level error type for the
//! signed-op apply path (`apply_local_signed_group_op` and friends). It
//! composes the domain enums via `#[from]` so apply-side code can write
//! `?` against any of them.

use thiserror::Error;

/// Errors raised by `MembershipRepository` and the membership-policy
/// layer. Covers admin/member predicates, last-admin protection, and
/// the inheritance-walk depth bound.
#[derive(Debug, Error)]
pub enum MembershipError {
    /// Identity is not the group's direct admin. Used by every
    /// admin-gated mutation in `MembershipRepository`,
    /// `CapabilitiesRepository::set_*`, and `PermissionChecker`.
    #[error("identity {identity} is not an admin of group {group_id}")]
    NotAdmin { group_id: String, identity: String },

    /// Removing this member would leave the group with zero admins.
    /// Hot path: `remove_member`, `MembershipPolicy::ensure_not_last_admin_removal`.
    #[error("cannot remove the last admin of the group")]
    LastAdmin,

    /// Demoting this member would leave the group with zero admins.
    #[error("cannot demote the last admin of the group")]
    LastAdminDemotion,

    /// Caller is not a member of the requested group (direct or
    /// inherited). Used in apply-time member-only ops like `MemberLeft`.
    #[error("identity {identity} is not a member of group {group_id}")]
    NotMember { group_id: String, identity: String },

    /// Stored value missing for an existing key. Indicates store
    /// corruption — distinct from "not found" (no key exists) and
    /// surfaced separately so callers can alert rather than retry.
    #[error("member key exists but value is missing for {identity} in group {group_id}")]
    MissingMemberValue { group_id: String, identity: String },

    /// Member row doesn't exist for the requested
    /// `(group_id, identity)` pair. Distinct from `NotMember`
    /// (which is about inheritance) — this is a direct-row miss
    /// used by mutation paths like `set_auto_follow`.
    #[error("member {member} not found in group {group_id}")]
    MemberNotFound { group_id: String, member: String },

    /// Identity is not a TEE-attestation verifier authoritative for
    /// this namespace.
    #[error("identity {0} is not authoritative for this namespace")]
    NotAuthoritative(String),

    /// TEE attestation submitted by a non-member. The verifier must
    /// itself be a member of the group whose admission policy it
    /// validates.
    #[error("TEE attestation verifier must be a group member")]
    TeeVerifierNotMember,

    /// `MemberJoinedViaTeeAttestation` was applied against a group
    /// that has no `TeeAdmissionPolicySet` op on record. Without a
    /// policy there's nothing to attest against — caller must set
    /// the policy first.
    #[error("MemberJoinedViaTeeAttestation rejected: no TeeAdmissionPolicySet exists for group")]
    NoTeeAdmissionPolicy,

    /// Inheritance walk hit the namespace depth bound — either the
    /// store has a cycle or the chain genuinely exceeds
    /// `MAX_NAMESPACE_DEPTH`.
    #[error("membership walk exceeded MAX_NAMESPACE_DEPTH ({0}); possible cycle in store")]
    DepthExceeded(usize),

    /// `MemberLeft` op signer doesn't match the leaving member —
    /// `MemberLeft` is self-leave only.
    #[error("MemberLeft is self-leave only: signer must equal the leaving member")]
    SelfLeaveOnly,

    /// `set_member_auto_follow` requires the caller to be either
    /// admin or the target member themselves.
    #[error("only group admin or the target member can set auto-follow")]
    AutoFollowAuthFailed,

    /// `ReadOnlyTee` role can only be set via
    /// `MemberJoinedViaTeeAttestation`, never via `MemberAdded` /
    /// `MemberRoleChanged`.
    #[error("ReadOnlyTee can only be assigned via MemberJoinedViaTeeAttestation")]
    ReadOnlyTeeViaAttestationOnly,

    /// `MemberJoinedViaTeeAttestation` must specify the `ReadOnlyTee`
    /// role — any other role is rejected.
    #[error("MemberJoinedViaTeeAttestation must use ReadOnlyTee role")]
    TeeRoleMustBeReadOnly,

    /// Cannot remove the owner of a group; the owner must
    /// `TransferOwnership` to a successor first.
    #[error("cannot remove owner of group {0}; owner must transfer ownership first")]
    OwnerImmuneFromRemoval(String),

    /// Member is not a *direct* member of the group — they reach it
    /// via inheritance. `MemberLeft` applies only to the direct
    /// anchor; the caller must leave the parent where the anchor lives.
    #[error(
        "member is not a direct member of group {0}; leave the parent group where the \
         membership anchor lives"
    )]
    MemberNotDirect(String),

    /// Owner cannot self-leave a group; transfer ownership first.
    #[error("owner of group {0} cannot self-leave; transfer ownership to a successor first")]
    OwnerCannotSelfLeave(String),

    /// Cannot leave a namespace while owning any subgroup in the
    /// subtree.
    #[error("cannot leave namespace: leaver owns subgroup {0}; transfer ownership first")]
    OwnerOwnsSubgroup(String),

    /// Only the current owner can transfer ownership of a group.
    #[error("only the current owner of group {0} can transfer ownership")]
    OnlyOwnerCanTransfer(String),

    /// Only the current owner can delete a group. Distinct from
    /// [`OnlyOwnerCanTransfer`] so callers can route delete-rejection
    /// to a different code path than transfer-rejection (e.g. an HTTP
    /// handler returning different error codes).
    #[error(
        "only the owner of group {0} can delete it; transfer ownership first if a \
         non-owner needs to remove it"
    )]
    OnlyOwnerCanDelete(String),

    /// Group does not exist (used for mutation paths that need the
    /// meta row — `TransferOwnership`, `GroupDeleted`, etc.). The
    /// apply-path's hash-recomputation has its own [`MetaError::
    /// GroupNotFoundForHash`] so the two recovery paths can diverge.
    #[error("group {0} not found for this action")]
    UnknownGroup(String),

    /// Context state-delta apply addressed a context that isn't
    /// registered in the named group.
    #[error("context {context_id} is not registered in group {group_id}")]
    ContextNotInGroup {
        group_id: String,
        context_id: String,
    },
}

/// Errors raised by `NamespaceRepository` and the namespace-DAG
/// services. Covers tree-structure invariants (cycles, depth, parent
/// edges) and identity lookups.
#[derive(Debug, Error)]
pub enum NamespaceError {
    /// Proposed nest would create a cycle in the parent chain.
    #[error("nesting would create a cycle")]
    NestingCycle,

    /// Self-parent: child == parent.
    #[error("cannot nest a group under itself")]
    SelfNesting,

    /// Group already has a parent; must `unnest` first.
    #[error("group {0} already has a parent; unnest it first")]
    AlreadyHasParent(String),

    /// Nesting beyond `MAX_NAMESPACE_DEPTH` — tree would be
    /// unwalkable by every other parent-chain operation.
    #[error("nesting depth exceeds MAX_NAMESPACE_DEPTH; tree would be unwalkable")]
    DepthExceeded,

    /// No `NamespaceIdentity` row stored for the namespace root.
    #[error("namespace identity not found for {0}")]
    NoNamespaceIdentity(String),

    /// Group is not within the namespace rooted at the expected
    /// ancestor. Used by `is_descendant_of` callers that bail when
    /// the relation is required (e.g. ownership-proof verification).
    #[error("group {child} is not a descendant of {ancestor}")]
    NotDescendant { ancestor: String, child: String },

    /// Group does not belong to the namespace it was looked up
    /// under. Hot path: tests that walk descendant lists.
    #[error("group does not belong to this namespace")]
    WrongNamespace,

    /// Reparent target's new parent is itself a descendant of the
    /// child — would create a cycle.
    #[error("cycle: new_parent {new_parent} is a descendant of child {child}")]
    ReparentCycle { new_parent: String, child: String },

    /// Cannot reparent the namespace root itself.
    #[error("cannot reparent the namespace root: {0} has no parent")]
    RootHasNoParent(String),

    /// Target of reparent is not in the same namespace.
    #[error("new parent group {0} not found in this namespace")]
    ReparentTargetMissing(String),

    /// Namespace root group not found at all.
    #[error("namespace root group not found")]
    RootMissing,

    /// Cannot use `GroupDeleted` to delete the namespace root group —
    /// use the dedicated `delete_namespace` op.
    #[error("cannot delete the namespace root {0}; use delete_namespace instead")]
    CannotDeleteRoot(String),

    /// `GroupCreated` op rejected because group_id == parent_id.
    /// Namespace roots are recorded separately, never via
    /// `GroupCreated`.
    #[error(
        "GroupCreated rejected: self-parent edge (group_id == parent_id); \
         namespace roots are recorded via the namespace-identity setup path"
    )]
    SelfParentEdge,

    /// Context state-delta apply rejected because the context resolves
    /// to a `ReadOnlyTee` subgroup — only TEE attestations can move
    /// state.
    #[error("context is in a ReadOnlyTee subgroup; only TEE attestations may write")]
    ReadOnlyTee,

    /// `TeeAdmissionPolicySet` rejected because it was emitted on a
    /// subgroup — policies are namespace-scoped, set on the root.
    #[error(
        "TeeAdmissionPolicySet rejected on subgroup {0}: policy is namespace-scoped, \
         set it on the namespace root"
    )]
    TeePolicyNotOnSubgroup(String),
}

/// Errors raised by `CapabilitiesRepository` and the higher-level
/// `PermissionChecker`. Used for the "you can't do that operation in
/// this group" class of failures, including capability-flag checks.
#[derive(Debug, Error)]
pub enum CapabilitiesError {
    /// Requester lacks the capability or admin role required for
    /// `operation`. The string `operation` is a stable identifier
    /// like `"set target application"` or `"manage metadata"` —
    /// callers can match on it for routing-level decisions.
    #[error("requester lacks permission to {operation} in group {group_id}")]
    Unauthorized { group_id: String, operation: String },
}

/// Errors raised by `SigningKeysRepository`. Distinct from the
/// crypto-layer errors raised by `GroupKeyring` because the
/// "no signing key" case is a *configuration* failure (caller never
/// registered one), not a cryptographic one.
#[derive(Debug, Error)]
pub enum SigningKeysError {
    /// No signing key stored for `identity` in `group_id`.
    #[error("signing key not found for {identity} in group {group_id}")]
    NotFound { group_id: String, identity: String },
}

/// Errors raised by `GroupKeyring` (encryption-key management) and
/// the AES-GCM / ECDH primitives it wraps. These are cryptographic
/// failures, not configuration ones — most map to a specific
/// underlying failure mode in the crypto layer.
#[derive(Debug, Error)]
pub enum KeyringError {
    /// No active group key stored. Caller should issue a key rotation.
    #[error("no group key stored for group {0}")]
    NoGroupKey(String),

    /// AES-GCM encryption failed (almost always: nonce reuse or
    /// internal-state corruption — both indicate a bug).
    #[error("AES-GCM encryption failed")]
    EncryptionFailed,

    /// AES-GCM decryption failed. The two most common causes are
    /// wrong sender_key (key-rotation race) and ciphertext corruption.
    #[error("failed to decrypt group op (bad sender_key or corrupt)")]
    DecryptionFailed,

    /// Key envelope decrypted to the wrong length. Indicates the
    /// envelope was constructed against a different key schema.
    #[error("decrypted key envelope has wrong length: {0}")]
    BadKeyLength(usize),

    /// ECDH key agreement failed. Underlying error preserved in
    /// `details` for diagnostics.
    #[error("ECDH key agreement failed: {details}")]
    KeyAgreementFailed { details: String },

    /// Borsh decode of decrypted `GroupOp` failed — almost always a
    /// schema-mismatch between sender and receiver.
    #[error("borsh decode inner GroupOp: {0}")]
    InnerOpDecodeFailed(String),
}

/// Errors raised by `MetaRepository`. Currently apply-path-specific —
/// the general "group not found" case for mutation paths lives on
/// [`MembershipError::UnknownGroup`] because those sites are
/// authorization-gated and route differently.
#[derive(Debug, Error)]
pub enum MetaError {
    /// Group not found during state-hash computation. Apply-path
    /// only — surfaces as a "diverged peer" signal rather than a
    /// generic "missing data" error.
    #[error("group not found for state hash computation")]
    GroupNotFoundForHash,
}

/// Errors raised by `UpgradesRepository` and the upgrade-orchestration
/// layer.
#[derive(Debug, Error)]
pub enum UpgradesError {
    /// An upgrade is already in progress for this group; concurrent
    /// upgrades would race the propagator state machine.
    #[error("an upgrade is already in progress for this group")]
    InProgress,

    /// Cannot delete a group while contexts are still registered.
    /// (Sits on upgrades because group deletion happens inside the
    /// upgrade-orchestration code path; could move to `MetaError`
    /// if a non-upgrade group-deletion code path is ever added.)
    #[error("cannot delete group: one or more contexts are still registered")]
    HasRegisteredContexts,
}

/// Errors raised by `ContextRegistrationRepository` (the context-to-group
/// indirection).
#[derive(Debug, Error)]
pub enum ContextRegistrationError {
    /// Context not registered to any group.
    #[error("context is not registered in any group")]
    NotRegistered,

    /// Context registered to a different group than expected — caller
    /// passed the wrong group_id or the registration changed under
    /// them.
    #[error("context is registered to a different group")]
    WrongGroup,
}

/// Errors raised on the `SignedGroupOp` apply path. These cross
/// Repository boundaries and so live as their own type, composing the
/// domain enums via `#[from]` so apply-time code can `?` any of them.
#[derive(Debug, Error)]
pub enum ApplyError {
    #[error(transparent)]
    Membership(#[from] MembershipError),

    #[error(transparent)]
    Namespace(#[from] NamespaceError),

    #[error(transparent)]
    Capabilities(#[from] CapabilitiesError),

    #[error(transparent)]
    SigningKeys(#[from] SigningKeysError),

    #[error(transparent)]
    Keyring(#[from] KeyringError),

    #[error(transparent)]
    Meta(#[from] MetaError),

    #[error(transparent)]
    Upgrades(#[from] UpgradesError),

    #[error(transparent)]
    ContextRegistration(#[from] ContextRegistrationError),

    /// State-hash mismatch between op-signed-against and current
    /// store state. Apply-path-only — outside the apply path the
    /// hashes aren't compared.
    #[error("state_hash mismatch: op signed against {expected}, current is {actual}")]
    StateHashMismatch { expected: String, actual: String },

    /// Op nonce <= last-processed nonce. Idempotent from the caller's
    /// perspective; callers that care (e.g. governance broadcast) can
    /// match on this variant and treat it as success.
    #[error("nonce {nonce} already processed (last: {last})")]
    StaleNonce { nonce: u64, last: u64 },

    /// `GroupOp` variant not supported on this code path (e.g. a
    /// remote-only op delivered to the local-apply handler).
    #[error("unsupported group op variant")]
    UnsupportedOp,

    /// Governance nonce counter overflowed `u64`. Practically
    /// unreachable; documented for completeness.
    #[error("group governance nonce overflow")]
    NonceOverflow,

    /// Namespace governance DAG heads exceeded
    /// `MAX_GOVERNANCE_DAG_HEADS` — back-pressure signal to the
    /// gossip layer.
    #[error("MAX_GOVERNANCE_DAG_HEADS exceeded for namespace")]
    DagHeadsExceeded,

    /// `GroupCreated` op rejected. The wrapped reason is a free-form
    /// description; callers that need a specific check should match
    /// the `NamespaceError` / `CapabilitiesError` variants instead,
    /// which carry structured fields. This variant exists for the
    /// catch-all paths where multiple distinct rejection causes share
    /// one log site.
    #[error("GroupCreated rejected: {reason}")]
    GroupCreatedRejected { reason: String },

    /// `GroupDeleted` op rejected, including cascade-divergence
    /// between the local subtree and the op payload.
    #[error("GroupDeleted rejected: {reason}")]
    GroupDeletedRejected { reason: String },

    /// `MemberJoinedOpen` op rejected. Common causes: signer ≠
    /// member, group_id resolves to a different namespace, signer
    /// already a direct member.
    #[error("MemberJoinedOpen rejected: {reason}")]
    MemberJoinedOpenRejected { reason: String },
}
