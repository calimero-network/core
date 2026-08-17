//! The one **op** envelope for the unified causal log.
//!
//! Every change — a data write, a writer-set rotation, a membership change, an
//! admin/policy change — is the same [`Op`], carried by the generic
//! `CausalDelta<T>` / `DagStore<T>` transport. A scope's state is the
//! deterministic projection of its op-log (see `calimero-projection`); its
//! single [`scope_root`] is the only convergence signal; authorization is one
//! fold over the op's causal cut (see `calimero-authz`).
//!
//! This crate is the small foundation: the op types plus the canonical id and
//! root hashing.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};

use calimero_account::{AccountGenesis, AccountId, DeviceCert, DeviceId, RootKeyHandoff};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::entities::OpMask;
use calimero_storage::logical_clock::HybridTimestamp;

/// Stable id of a **visibility scope** — one node in a context's scope tree
/// (root governance scope, a context, a subgroup, …). Each scope is a
/// self-contained replication + encryption + convergence domain with its own
/// op-log, key, members, and [`scope_root`]. Convergence is always per scope.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, BorshSerialize, BorshDeserialize,
)]
pub struct ScopeId([u8; 32]);

impl ScopeId {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for ScopeId {
    fn from(value: [u8; 32]) -> Self {
        Self(value)
    }
}

/// Who authored an op, as one indivisible triple.
///
/// These three answer three *different* questions that used to be answered by a
/// single key, and separating them is what makes one identity across several
/// devices possible:
///
/// - [`account`](Self::account) — **who**, for authorization and for the app.
///   The only subject the ACL and membership planes key on.
/// - [`device`](Self::device) — **which replica**, for the CRDT planes. Must be
///   unique per concurrent writer; never an authorization input.
/// - [`device_key`](Self::device_key) — **what signed this**, for integrity.
///
/// They travel together because a claim is only meaningful as a unit: the
/// signature proves the device key authored the op, and the projection proves
/// that key currently speaks for that account. Splitting them across call
/// boundaries invites checking one without the other.
///
/// All three are covered by [`Op::compute_id`], hence by the signature. If
/// `device_key` were left out, an attacker could swap in their own key; if
/// `account` were left out, a device's op could be replayed under a different
/// account.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct Authorship {
    /// The authorizing identity — what ACLs, membership, and the app see.
    ///
    /// **Carried on the op, and it has to be.** The obvious simplification is to
    /// drop this and resolve the author from the folded device bindings instead —
    /// the membership plane already does exactly that, in
    /// `ScopeProjections::account_for_author`. It does not work here, and the
    /// reason is recorded in `calimero-projection`'s fold: a projection folds
    /// **raw logs** (`from_ops` and the sync convergence path both do), so a
    /// binding read mid-fold answers "has that device's link folded *yet*"
    /// rather than "what does this key speak for". A revoked device once stored
    /// either the binding's account or the payload's claim depending on which
    /// arrived first, and that value is hashed into `governance_hash` — so it
    /// **split the root by arrival order**.
    ///
    /// The membership plane escapes this because it resolves against a *fixed
    /// cut*, where the answer is a function of the op set rather than its order.
    /// The fold has no cut to resolve against; it is what builds one.
    ///
    /// So a producer establishes this before the op is folded — from a credential
    /// the op carries, or from a binding it resolved while applying — and an op
    /// nothing can attribute gets [`Self::UNATTRIBUTED_ACCOUNT`] rather than an
    /// invented account or a `None` every consumer must remember to handle.
    pub account: AccountId,
    /// The CRDT replica id of the installation that authored this.
    pub device: DeviceId,
    /// The Ed25519 key that produced [`Op::signature`].
    pub device_key: PublicKey,
}

impl Authorship {
    /// The account id that names **nobody** — the answer for an op whose author
    /// cannot be established.
    ///
    /// A key becomes attributable through a credential the op carries or a
    /// binding a producer resolved. When neither exists there is no principal to
    /// name, and the honest record of that is one well-known value rather than a
    /// per-key derivation. The old stand-in hashed the signing key into an
    /// account-shaped id, which read as a real principal at every call site that
    /// saw it, and *looked* different for every key — so "we could not attribute
    /// this" was indistinguishable from "this is somebody".
    ///
    /// Every gate on this plane asks whether the author equals some real
    /// principal, so a value no genesis can produce **fails closed everywhere by
    /// construction**: membership, admin, ownership and the account-plane
    /// handoff check all answer "no" without any of them needing a special case.
    /// That is why this is a sentinel and not an `Option` — an option would put
    /// the same decision in a dozen places and rely on each getting it right.
    /// See [`Authorship::account`] for why the field is carried at all rather
    /// than resolved during the fold.
    pub const UNATTRIBUTED_ACCOUNT: AccountId = AccountId::from_raw([0u8; 32]);

    /// The device id paired with [`Self::UNATTRIBUTED_ACCOUNT`].
    pub const UNATTRIBUTED_DEVICE: DeviceId = DeviceId::from_raw([0u8; 32]);

    /// Authorship for an op whose author could not be established.
    ///
    /// `device_key` is still recorded, because it is a fact: that key signed the
    /// op, and `Op::verify` checks the signature against it. Only the principal
    /// it speaks for is unknown.
    #[must_use]
    pub const fn unattributed(device_key: PublicKey) -> Self {
        Self {
            account: Self::UNATTRIBUTED_ACCOUNT,
            device: Self::UNATTRIBUTED_DEVICE,
            device_key,
        }
    }
}

/// One envelope for every kind of change in a scope.
///
/// `parents` are the op's causal predecessors **within its scope**, and MAY
/// also include a cross-scope governance head the op was authored under
/// (visibility-respecting: a subgroup op may reference its ancestor governance
/// scope's head, since subgroup members are ancestor members). It is one parent
/// set, one causal model, spanning data and governance.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct Op {
    /// `compute_id(scope, parents, authorship, hlc, payload)` — content
    /// address. **Private** and computed by [`Op::new`] so a caller can't
    /// desync the id from the content it addresses; read it via [`Op::id`] and
    /// re-check it with [`Op::verify`].
    id: [u8; 32],
    /// The scope this op belongs to.
    pub scope: ScopeId,
    /// Causal predecessors (may cross scopes — see the struct docs).
    pub parents: Vec<[u8; 32]>,
    /// Who authored this: account, device, and signing key. See [`Authorship`]
    /// for why all three are carried rather than one key doing every job.
    pub authorship: Authorship,
    /// Hybrid logical clock at author time (causally monotonic).
    pub hlc: HybridTimestamp,
    /// The change itself. Once payload encryption lands, the data arms are
    /// ciphertext at rest under the scope key.
    pub payload: OpPayload,
    /// The author's expected `scope_root` after applying this op — a
    /// convergence **assertion**, not a trusted input. Deliberately NOT part of
    /// the [`compute_id`](Op::compute_id) preimage (so it is unsigned): peers
    /// **recompute** their own `scope_root` from their projection and compare,
    /// rather than trusting the author's number. A tampered value cannot grant
    /// authority — at worst it flags a divergence the recompute would catch
    /// anyway. Security never depends on this field.
    pub expected_scope_root: [u8; 32],
    /// Ed25519 signature by [`Authorship::device_key`] over the
    /// [`compute_id`](Op::compute_id) preimage (i.e. over `id`). The signature
    /// is intentionally NOT folded back into `id` (it signs the id, which would
    /// be circular).
    ///
    /// **Callers MUST verify this signature before trusting an `Op`.**
    /// `calimero-projection`/`calimero-authz` assume already-verified ops: they
    /// fold/authorize on content alone and perform no signature check. Feeding
    /// an unverified op into the projection bypasses authentication entirely.
    ///
    /// Note the two-stage split. Verifying this signature proves only that the
    /// holder of `device_key` authored the op — it says nothing about whether
    /// that key currently speaks for [`Authorship::account`]. That second
    /// question is answered at the causal cut by `calimero-authz`, because only
    /// the cut knows which links and revocations are in force.
    pub signature: [u8; 64],
}

/// The change an [`Op`] carries, across all four planes folded into one model.
///
/// **Append-only wire format.** An op's content-address [`id`](Op::id) is a hash
/// over `borsh(payload)`, and borsh encodes an enum variant by its *positional*
/// discriminant (declaration order → tag byte). Inserting a variant in the
/// middle, removing one, or reordering therefore renumbers every later variant,
/// which silently changes the id — and thus the signature — of every already
/// stored/persisted op that used one of the shifted variants. New variants MUST
/// be appended at the end only; existing variants must never be reordered or
/// removed. [`op_payload_discriminants_are_pinned`] guards this.
///
/// This enum is intentionally *not* `#[non_exhaustive]`: `calimero-authz`
/// authorizes ops with an exhaustive match over `OpPayload`, so a newly added
/// variant should fail to compile there until it is explicitly given an
/// authorization rule, rather than being swept into a catch-all arm.
///
/// [`op_payload_discriminants_are_pinned`]: tests::op_payload_discriminants_are_pinned
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

impl Op {
    /// Build an op, computing its content-address [`id`](Op::id) from the
    /// content so the two can never disagree. `signature` is the author's
    /// Ed25519 signature over that id (see the [`signature`](Op::signature)
    /// field docs); callers sign `Op::compute_id(...)` with the author key.
    #[must_use]
    pub fn new(
        scope: ScopeId,
        parents: Vec<[u8; 32]>,
        authorship: Authorship,
        hlc: HybridTimestamp,
        payload: OpPayload,
        expected_scope_root: [u8; 32],
        signature: [u8; 64],
    ) -> Self {
        let id = Self::compute_id(scope, &parents, &authorship, &hlc, &payload);
        Self {
            id,
            scope,
            parents,
            authorship,
            hlc,
            payload,
            expected_scope_root,
            signature,
        }
    }

    /// Build an op from an **explicit** `id` rather than recomputing it from the
    /// content.
    ///
    /// This exists only for the unified-op *bridge*: a [`SignedNamespaceOp`] /
    /// rotation entry is already a node in the governance DAG with its own
    /// identity (`content_hash` / `delta_id`), and the unified `Op` mirrors that
    /// node verbatim — keyed in the op-store by that same id — rather than by
    /// `Op::compute_id` of the projected payload. These bridge ops are internal,
    /// unsigned projections of already-verified governance ops, so they are not
    /// passed through [`Op::verify`]. Fresh, independently-signed ops must use
    /// [`Op::new`] instead, so their id is a true content address.
    #[expect(
        clippy::too_many_arguments,
        reason = "one parameter per Op field (incl. the explicit id); a builder \
                  would obscure the deliberate 1:1 field mapping for the bridge"
    )]
    #[must_use]
    pub fn from_parts(
        id: [u8; 32],
        scope: ScopeId,
        parents: Vec<[u8; 32]>,
        authorship: Authorship,
        hlc: HybridTimestamp,
        payload: OpPayload,
        expected_scope_root: [u8; 32],
        signature: [u8; 64],
    ) -> Self {
        Self {
            id,
            scope,
            parents,
            authorship,
            hlc,
            payload,
            expected_scope_root,
            signature,
        }
    }

    /// The account whose authority this op is judged against.
    #[must_use]
    pub const fn author(&self) -> AccountId {
        self.authorship.account
    }

    /// The CRDT replica that authored this op.
    #[must_use]
    pub const fn device(&self) -> DeviceId {
        self.authorship.device
    }

    /// The key that signed this op.
    #[must_use]
    pub const fn device_key(&self) -> &PublicKey {
        &self.authorship.device_key
    }

    /// Content address of this op.
    #[must_use]
    pub const fn id(&self) -> [u8; 32] {
        self.id
    }

    /// Verify this op end-to-end: the cached [`id`](Op::id) actually addresses
    /// the content, **and** the signature is a valid Ed25519 signature over
    /// that id by [`author`](Op::author).
    ///
    /// `calimero-projection`/`calimero-authz` assume already-verified ops, so
    /// every op crossing a trust boundary (deserialized, received from a peer)
    /// MUST pass this before being folded.
    #[must_use]
    pub fn verify(&self) -> bool {
        let recomputed = Self::compute_id(
            self.scope,
            &self.parents,
            &self.authorship,
            &self.hlc,
            &self.payload,
        );
        if recomputed != self.id {
            return false;
        }
        // Against the DEVICE key, not the account: an account has no key of its
        // own to sign with, and the whole point of the split is that the thing
        // which signs is per-device and revocable. Binding that key to
        // `authorship.account` is a separate, at-cut decision made by
        // `calimero-authz` — see the `signature` field docs.
        self.authorship
            .device_key
            .verify_raw_signature(&self.id, &self.signature)
            .is_ok()
    }

    /// Content address of an op: `Sha256(scope ‖ sorted(parents) ‖
    /// borsh(authorship) ‖ hlc ‖ borsh(payload))`. Parents are sorted so the id
    /// is independent of the order a builder happened to list them in.
    ///
    /// The whole [`Authorship`] triple is in the preimage, so the account, the
    /// replica id, and the signing key are all covered by the signature. Each
    /// omission would be exploitable on its own — see the [`Authorship`] docs.
    ///
    /// # Panics
    /// Never in practice — borsh-serializing these field types into an
    /// in-memory buffer is infallible; the `expect` documents that invariant.
    #[must_use]
    pub fn compute_id(
        scope: ScopeId,
        parents: &[[u8; 32]],
        authorship: &Authorship,
        hlc: &HybridTimestamp,
        payload: &OpPayload,
    ) -> [u8; 32] {
        let mut sorted = parents.to_vec();
        sorted.sort_unstable();

        let mut hasher = Sha256::new();
        hasher.update(scope.as_bytes());
        // Length-prefix the parent list so the boundary between the (variable
        // count of) parents and the authorship that follows is unambiguous —
        // i.e. `parents=[A,B], account=C` can never hash-collide with
        // `parents=[A,B,C], account=…`. All other fields are fixed-size or
        // borsh-length-prefixed.
        hasher.update((sorted.len() as u64).to_le_bytes());
        for parent in &sorted {
            hasher.update(parent);
        }
        hasher.update(borsh::to_vec(authorship).expect("Authorship borsh is infallible in-memory"));
        hasher.update(borsh::to_vec(hlc).expect("HybridTimestamp borsh is infallible in-memory"));
        hasher.update(borsh::to_vec(payload).expect("OpPayload borsh is infallible in-memory"));
        hasher.finalize().into()
    }
}

/// The single convergence root over a scope's **whole** projection —
/// values **and** authorization (ACL + groups). Folding the ACL/membership in
/// is what makes a hash-neutral writer/membership rotation impossible to hide:
/// a divergent writer set is a divergent root, so sync can never declare
/// "done" while the authorization state disagrees.
///
/// Combining function only; `calimero-projection` computes the three component
/// hashes from a `ScopeState`.
#[must_use]
pub fn scope_root(entities_root: [u8; 32], acl_hash: [u8; 32], groups_root: [u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(entities_root);
    hasher.update(acl_hash);
    hasher.update(groups_root);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic keypair, so failures reproduce exactly.
    fn key(seed: u8) -> calimero_primitives::identity::PrivateKey {
        calimero_primitives::identity::PrivateKey::from([seed; 32])
    }

    /// A real (non-self) account with one device, for authorship tests.
    fn real_authorship(root_seed: u8, dev_seed: u8) -> Authorship {
        let account = AccountGenesis::new(key(root_seed).public_key()).account_id();
        Authorship {
            account,
            device: DeviceId::mint(account, [dev_seed; 16]),
            device_key: key(dev_seed).public_key(),
        }
    }

    fn hlc0() -> HybridTimestamp {
        use core::num::NonZeroU128;

        use calimero_storage::logical_clock::{Timestamp, ID, NTP64};
        HybridTimestamp::new(Timestamp::new(
            NTP64(0),
            ID::from(NonZeroU128::new(1).unwrap()),
        ))
    }

    #[test]
    fn compute_id_is_parent_order_invariant() {
        let scope = ScopeId::from([7u8; 32]);
        let author = real_authorship(1, 2);
        let hlc = hlc0();
        let payload = OpPayload::Delete {
            entity: Id::new([2u8; 32]),
        };
        let a = Op::compute_id(scope, &[[3u8; 32], [4u8; 32]], &author, &hlc, &payload);
        let b = Op::compute_id(scope, &[[4u8; 32], [3u8; 32]], &author, &hlc, &payload);
        assert_eq!(a, b, "id must not depend on parent ordering");
    }

    #[test]
    fn compute_id_distinguishes_payload() {
        let scope = ScopeId::from([7u8; 32]);
        let author = real_authorship(1, 2);
        let hlc = hlc0();
        let put = OpPayload::Put {
            entity: Id::new([2u8; 32]),
            value: vec![1, 2, 3],
        };
        let del = OpPayload::Delete {
            entity: Id::new([2u8; 32]),
        };
        assert_ne!(
            Op::compute_id(scope, &[], &author, &hlc, &put),
            Op::compute_id(scope, &[], &author, &hlc, &del),
        );
    }

    #[test]
    fn scope_root_combines_all_three_components() {
        let base = scope_root([0u8; 32], [0u8; 32], [0u8; 32]);
        // Changing ANY component (entities, acl, or groups) moves the root —
        // the property that makes a hash-neutral ACL rotation impossible.
        assert_ne!(base, scope_root([1u8; 32], [0u8; 32], [0u8; 32]));
        assert_ne!(base, scope_root([0u8; 32], [1u8; 32], [0u8; 32]));
        assert_ne!(base, scope_root([0u8; 32], [0u8; 32], [1u8; 32]));
    }

    #[test]
    fn compute_id_covers_every_part_of_authorship() {
        // Each field is separately exploitable if unsigned: swapping the
        // account replays a device's op under someone else, swapping the
        // device forges a replica id, swapping the key substitutes the signer.
        let scope = ScopeId::from([7u8; 32]);
        let hlc = hlc0();
        let payload = OpPayload::Delete {
            entity: Id::new([2u8; 32]),
        };
        let base = real_authorship(1, 2);
        let id = |a: &Authorship| Op::compute_id(scope, &[], a, &hlc, &payload);

        let mut other_account = base;
        other_account.account = AccountId::from([42u8; 32]);
        assert_ne!(id(&base), id(&other_account), "account must be signed");

        let mut other_device = base;
        other_device.device = DeviceId::from([43u8; 32]);
        assert_ne!(id(&base), id(&other_device), "device must be signed");

        let mut other_key = base;
        other_key.device_key = key(9).public_key();
        assert_ne!(id(&base), id(&other_key), "device_key must be signed");
    }

    #[test]
    fn verify_checks_the_signature_against_the_device_key() {
        // An account has no key of its own; the device key is what signs.
        let device_sk = key(2);
        let authorship = real_authorship(1, 2);
        let scope = ScopeId::from([7u8; 32]);
        let payload = OpPayload::Put {
            entity: Id::new([2u8; 32]),
            value: vec![1],
        };
        let id = Op::compute_id(scope, &[], &authorship, &hlc0(), &payload);
        let op = Op::new(
            scope,
            vec![],
            authorship,
            hlc0(),
            payload,
            [0u8; 32],
            device_sk.sign(&id).expect("sign").to_bytes(),
        );
        assert!(op.verify());
        assert_eq!(op.author(), authorship.account);
        assert_eq!(op.device(), authorship.device);
        assert_eq!(*op.device_key(), device_sk.public_key());
    }

    #[test]
    fn verify_rejects_an_op_signed_by_a_different_device_key() {
        let authorship = real_authorship(1, 2);
        let scope = ScopeId::from([7u8; 32]);
        let payload = OpPayload::Put {
            entity: Id::new([2u8; 32]),
            value: vec![1],
        };
        let id = Op::compute_id(scope, &[], &authorship, &hlc0(), &payload);
        // Signed by key 9 while claiming device_key of key 2.
        let op = Op::new(
            scope,
            vec![],
            authorship,
            hlc0(),
            payload,
            [0u8; 32],
            key(9).sign(&id).expect("sign").to_bytes(),
        );
        assert!(!op.verify());
    }

    #[test]
    fn verify_rejects_a_swapped_account_after_signing() {
        // The account is in the id preimage, so re-pointing a validly signed op
        // at another account breaks the id/content match before the signature
        // check is even reached.
        let device_sk = key(2);
        let authorship = real_authorship(1, 2);
        let scope = ScopeId::from([7u8; 32]);
        let payload = OpPayload::Put {
            entity: Id::new([2u8; 32]),
            value: vec![1],
        };
        let id = Op::compute_id(scope, &[], &authorship, &hlc0(), &payload);
        let mut op = Op::new(
            scope,
            vec![],
            authorship,
            hlc0(),
            payload,
            [0u8; 32],
            device_sk.sign(&id).expect("sign").to_bytes(),
        );
        assert!(op.verify());
        op.authorship.account = AccountId::from([99u8; 32]);
        assert!(!op.verify());
    }

    #[test]
    fn op_payload_discriminants_are_pinned() {
        use calimero_context_config::types::ContextGroupId;
        use calimero_context_config::MemberCapabilities;
        use calimero_primitives::context::GroupMemberRole;

        let id = Id::new([1u8; 32]);
        let pk = AccountId::from([2u8; 32]);
        let scope = ScopeId::from([3u8; 32]);
        let group = ContextGroupId::from([4u8; 32]);
        let caps = MemberCapabilities::empty();
        let genesis = AccountGenesis::new(key(1).public_key());
        let account = genesis.account_id();
        let device = DeviceId::mint(account, [1u8; 16]);
        let handoff = RootKeyHandoff {
            account,
            from_epoch: 0,
            new_root_sign_pk: key(2).public_key(),
            signature: [0u8; 64],
        };
        let cert = DeviceCert {
            account,
            device,
            sign_pk: key(3).public_key(),
            kem_pk: calimero_account::KemPublicKey::from([4u8; 32]),
            key_epoch: 0,
            device_epoch: 0,
            signature: [0u8; 64],
        };

        // Every variant, paired with the borsh discriminant it MUST keep forever
        // (see the append-only note on `OpPayload`). The exhaustive `match` below
        // means adding a variant fails to compile until it is appended here with
        // its own pinned tag — never inserted in the middle.
        let all = [
            OpPayload::Put {
                entity: id,
                value: vec![1],
            },
            OpPayload::Delete { entity: id },
            OpPayload::SetWriters {
                object: id,
                writers: BTreeMap::new(),
            },
            OpPayload::MemberAdded {
                group,
                member: pk,
                role: GroupMemberRole::Member,
            },
            OpPayload::MemberRemoved { group, member: pk },
            OpPayload::AdminChanged { new_admin: pk },
            OpPayload::PolicyUpdated {
                policy_bytes: vec![],
            },
            OpPayload::SubgroupCreated {
                child: scope,
                parent: scope,
                restricted: false,
                admin: pk,
            },
            OpPayload::SubgroupReparented {
                child: scope,
                new_parent: scope,
            },
            OpPayload::SubgroupDeleted { scope },
            OpPayload::SubgroupVisibilitySet {
                scope,
                restricted: true,
            },
            OpPayload::DefaultCapabilitiesSet {
                group,
                capabilities: caps,
            },
            OpPayload::MemberCapabilitySet {
                group,
                member: pk,
                capabilities: caps,
            },
            OpPayload::Noop,
            OpPayload::DeviceLinked {
                genesis,
                chain: vec![],
                cert,
            },
            OpPayload::DeviceRevoked { account, device },
            OpPayload::AccountKeysRotated { handoff },
            OpPayload::MemberJoinedWithDevice {
                group,
                member: account,
                role: GroupMemberRole::Member,
                genesis,
                chain: vec![],
                cert,
            },
        ];

        // Exhaustive: a new variant forces a new arm here.
        fn pinned_tag(p: &OpPayload) -> u8 {
            match p {
                OpPayload::Put { .. } => 0,
                OpPayload::Delete { .. } => 1,
                OpPayload::SetWriters { .. } => 2,
                OpPayload::MemberAdded { .. } => 3,
                OpPayload::MemberRemoved { .. } => 4,
                OpPayload::AdminChanged { .. } => 5,
                OpPayload::PolicyUpdated { .. } => 6,
                OpPayload::SubgroupCreated { .. } => 7,
                OpPayload::SubgroupReparented { .. } => 8,
                OpPayload::SubgroupDeleted { .. } => 9,
                OpPayload::SubgroupVisibilitySet { .. } => 10,
                OpPayload::DefaultCapabilitiesSet { .. } => 11,
                OpPayload::MemberCapabilitySet { .. } => 12,
                OpPayload::Noop => 13,
                OpPayload::DeviceLinked { .. } => 14,
                OpPayload::DeviceRevoked { .. } => 15,
                OpPayload::AccountKeysRotated { .. } => 16,
                OpPayload::MemberJoinedWithDevice { .. } => 17,
            }
        }

        assert_eq!(all.len(), 18, "every OpPayload variant must be listed");
        for payload in &all {
            let bytes = borsh::to_vec(payload).expect("serialize");
            assert_eq!(
                bytes[0],
                pinned_tag(payload),
                "borsh discriminant drifted for {payload:?} — variants must be append-only"
            );
        }
    }

    #[test]
    fn op_payload_borsh_roundtrips() {
        let payload = OpPayload::SetWriters {
            object: Id::new([5u8; 32]),
            writers: [(AccountId::from([9u8; 32]), OpMask::FULL)]
                .into_iter()
                .collect(),
        };
        let bytes = borsh::to_vec(&payload).unwrap();
        let decoded: OpPayload = borsh::from_slice(&bytes).unwrap();
        assert_eq!(payload, decoded);
    }

    #[test]
    fn op_payload_rejects_out_of_range_discriminant() {
        // 17 variants are pinned (tags 0..=16). Any higher tag must fail to
        // decode rather than silently map onto a variant — the guard that
        // keeps a corrupt/forward-version byte from being mistaken for a valid
        // op.
        for tag in [17u8, 18, 20, 42, 200, 255] {
            let bytes = [tag];
            assert!(
                borsh::from_slice::<OpPayload>(&bytes).is_err(),
                "discriminant {tag} must not decode to any OpPayload variant"
            );
        }
    }

    #[test]
    fn op_payload_rejects_truncated_bytes() {
        // Encode a real payload, then assert every strict prefix fails to
        // decode. A truncated buffer must error, never yield a partial value.
        let payload = OpPayload::SetWriters {
            object: Id::new([5u8; 32]),
            writers: [(AccountId::from([9u8; 32]), OpMask::FULL)]
                .into_iter()
                .collect(),
        };
        let bytes = borsh::to_vec(&payload).unwrap();
        for len in 0..bytes.len() {
            assert!(
                borsh::from_slice::<OpPayload>(&bytes[..len]).is_err(),
                "truncated OpPayload ({len}/{} bytes) must not decode",
                bytes.len()
            );
        }
        // The full buffer still decodes (guards against an off-by-one that
        // would make the loop vacuous).
        assert!(borsh::from_slice::<OpPayload>(&bytes).is_ok());
    }

    #[test]
    fn op_payload_rejects_trailing_garbage() {
        // borsh requires the whole buffer to be consumed; extra bytes after a
        // complete payload must be rejected, not ignored.
        let payload = OpPayload::Delete {
            entity: Id::new([2u8; 32]),
        };
        let mut bytes = borsh::to_vec(&payload).unwrap();
        bytes.push(0xFF);
        assert!(
            borsh::from_slice::<OpPayload>(&bytes).is_err(),
            "trailing bytes after a complete payload must be rejected"
        );
    }

    #[test]
    fn op_rejects_malformed_bytes() {
        // Full `Op` decode over degenerate buffers must error cleanly rather
        // than panic on an unexpected EOF or a bogus length prefix.
        assert!(borsh::from_slice::<Op>(&[]).is_err());
        assert!(borsh::from_slice::<Op>(&[0u8; 8]).is_err());
        assert!(borsh::from_slice::<Op>(&[0xFFu8; 16]).is_err());
    }
}
