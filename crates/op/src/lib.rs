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

/// One envelope for every kind of change in a scope.
///
/// `parents` are the op's causal predecessors **within its scope**, and MAY
/// also include a cross-scope governance head the op was authored under
/// (visibility-respecting: a subgroup op may reference its ancestor governance
/// scope's head, since subgroup members are ancestor members). It is one parent
/// set, one causal model, spanning data and governance.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct Op {
    /// `compute_id(scope, parents, author, hlc, payload)` — content address.
    /// **Private** and computed by [`Op::new`] so a caller can't desync the id
    /// from the content it addresses; read it via [`Op::id`] and re-check it
    /// with [`Op::verify`].
    id: [u8; 32],
    /// The scope this op belongs to.
    pub scope: ScopeId,
    /// Causal predecessors (may cross scopes — see the struct docs).
    pub parents: Vec<[u8; 32]>,
    /// Authoring identity (verified against this scope's ACL at the op's cut).
    pub author: PublicKey,
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
    /// Ed25519 signature by `author` over the [`compute_id`](Op::compute_id)
    /// preimage (i.e. over `id`). The signature is intentionally NOT folded
    /// back into `id` (it signs the id, which would be circular).
    ///
    /// **Callers MUST verify this signature against `author` before trusting an
    /// `Op`.** `calimero-projection`/`calimero-authz` assume already-verified
    /// ops: they fold/authorize on content alone and perform no signature
    /// check. Feeding an unverified op into the projection bypasses
    /// authentication entirely.
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
        writers: BTreeMap<PublicKey, OpMask>,
    },

    // ---- membership plane ----
    /// Add `member` to `group` with `role`.
    MemberAdded {
        group: ContextGroupId,
        member: PublicKey,
        role: GroupMemberRole,
    },
    /// Remove `member` from `group`.
    MemberRemoved {
        group: ContextGroupId,
        member: PublicKey,
    },

    // ---- admin / namespace plane ----
    /// Change the scope's root admin.
    AdminChanged { new_admin: PublicKey },
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
        admin: PublicKey,
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
        member: PublicKey,
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
        author: PublicKey,
        hlc: HybridTimestamp,
        payload: OpPayload,
        expected_scope_root: [u8; 32],
        signature: [u8; 64],
    ) -> Self {
        let id = Self::compute_id(scope, &parents, &author, &hlc, &payload);
        Self {
            id,
            scope,
            parents,
            author,
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
        author: PublicKey,
        hlc: HybridTimestamp,
        payload: OpPayload,
        expected_scope_root: [u8; 32],
        signature: [u8; 64],
    ) -> Self {
        Self {
            id,
            scope,
            parents,
            author,
            hlc,
            payload,
            expected_scope_root,
            signature,
        }
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
            &self.author,
            &self.hlc,
            &self.payload,
        );
        if recomputed != self.id {
            return false;
        }
        self.author
            .verify_raw_signature(&self.id, &self.signature)
            .is_ok()
    }

    /// Content address of an op: `Sha256(scope ‖ sorted(parents) ‖ author ‖
    /// hlc ‖ borsh(payload))`. Parents are sorted so the id is independent of
    /// the order a builder happened to list them in.
    ///
    /// # Panics
    /// Never in practice — borsh-serializing these field types into an
    /// in-memory buffer is infallible; the `expect` documents that invariant.
    #[must_use]
    pub fn compute_id(
        scope: ScopeId,
        parents: &[[u8; 32]],
        author: &PublicKey,
        hlc: &HybridTimestamp,
        payload: &OpPayload,
    ) -> [u8; 32] {
        let mut sorted = parents.to_vec();
        sorted.sort_unstable();

        let mut hasher = Sha256::new();
        hasher.update(scope.as_bytes());
        // Length-prefix the parent list so the boundary between the (variable
        // count of) parents and the author that follows is unambiguous — i.e.
        // `parents=[A,B], author=C` can never hash-collide with
        // `parents=[A,B,C], author=…`. All other fields are fixed-size or
        // borsh-length-prefixed.
        hasher.update((sorted.len() as u64).to_le_bytes());
        for parent in &sorted {
            hasher.update(parent);
        }
        hasher.update(AsRef::<[u8; 32]>::as_ref(author));
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
        let author = PublicKey::from([1u8; 32]);
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
        let author = PublicKey::from([1u8; 32]);
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
    fn op_payload_discriminants_are_pinned() {
        use calimero_context_config::types::ContextGroupId;
        use calimero_context_config::MemberCapabilities;
        use calimero_primitives::context::GroupMemberRole;

        let id = Id::new([1u8; 32]);
        let pk = PublicKey::from([2u8; 32]);
        let scope = ScopeId::from([3u8; 32]);
        let group = ContextGroupId::from([4u8; 32]);
        let caps = MemberCapabilities::empty();

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
            }
        }

        assert_eq!(all.len(), 14, "every OpPayload variant must be listed");
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
            writers: [(PublicKey::from([9u8; 32]), OpMask::FULL)]
                .into_iter()
                .collect(),
        };
        let bytes = borsh::to_vec(&payload).unwrap();
        let decoded: OpPayload = borsh::from_slice(&bytes).unwrap();
        assert_eq!(payload, decoded);
    }
}
