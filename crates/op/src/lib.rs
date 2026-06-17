//! The one **op** envelope for the unified causal log (core#2716, Phase 5).
//!
//! Every change — a data write, a writer-set rotation, a membership change, an
//! admin/policy change — is the same [`Op`] carried by the existing generic
//! `CausalDelta<T>` / `DagStore<T>` transport. A scope's state is the
//! deterministic projection of its op-log (see `calimero-projection`); its
//! single [`scope_root`] is the only convergence signal; authorization is one
//! fold over the op's causal cut (see `calimero-authz`).
//!
//! This crate is the small foundation: the op types + the canonical id and
//! root hashing. It is **additive scaffolding** — the existing storage,
//! governance, and sync layers are migrated onto it in later Phase-5/6 stages.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};

use calimero_context_config::types::ContextGroupId;
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
pub struct ScopeId(pub [u8; 32]);

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
/// (design §3.3 — visibility-respecting: a subgroup op may reference its
/// ancestor governance scope's head, since subgroup members are ancestor
/// members). This is the unified successor to the data DAG's `parents` plus
/// the P4 `GovernanceParentEdge` — one parent set, one causal model.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct Op {
    /// `compute_id(scope, parents, author, hlc, payload)` — content address.
    pub id: [u8; 32],
    /// The scope this op belongs to.
    pub scope: ScopeId,
    /// Causal predecessors (may cross scopes, design §3.3).
    pub parents: Vec<[u8; 32]>,
    /// Authoring identity (verified against this scope's ACL at the op's cut).
    pub author: PublicKey,
    /// Hybrid logical clock at author time (causally monotonic, #2635).
    pub hlc: HybridTimestamp,
    /// The change itself (ciphertext at rest under the scope key for data arms
    /// once encryption lands; cleartext in this scaffold).
    pub payload: OpPayload,
    /// The author's expected `scope_root` after applying this op — a
    /// convergence **assertion**, not a trusted input. Deliberately NOT part of
    /// the [`compute_id`](Op::compute_id) preimage (so it is unsigned), exactly
    /// like the existing data DAG's `CausalDelta::expected_root_hash`: peers
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
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub enum OpPayload {
    // ---- data plane (was StateDelta actions) ----
    /// Write `value` to `entity`.
    Put { entity: Id, value: Vec<u8> },
    /// Delete `entity`.
    Delete { entity: Id },

    // ---- access-control plane (was RotationLogEntry / OpMask grants) ----
    /// Set the writer/capability set for `object` (writer-set rotation).
    SetWriters {
        object: Id,
        writers: BTreeMap<PublicKey, OpMask>,
    },

    // ---- membership plane (was SignedGroupOp / GroupOp) ----
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

    // ---- admin / namespace plane (was SignedNamespaceOp / NamespaceOp) ----
    /// Change the scope's root admin.
    AdminChanged { new_admin: PublicKey },
    /// Replace the scope's policy bytes.
    PolicyUpdated { policy_bytes: Vec<u8> },
    /// Create a child subgroup scope nested under `parent` (restricted ⇒
    /// member-only existence, design §3.4).
    SubgroupCreated {
        child: ScopeId,
        parent: ScopeId,
        restricted: bool,
    },
    /// Move a subgroup scope under a new parent (the scope-tree restructure
    /// `RootOp::GroupReparented`).
    SubgroupReparented { child: ScopeId, new_parent: ScopeId },
    /// Delete a subgroup scope (and, in the full model, its subtree — the
    /// caller emits one per cascaded scope; `RootOp::GroupDeleted`).
    SubgroupDeleted { scope: ScopeId },
}

impl Op {
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
