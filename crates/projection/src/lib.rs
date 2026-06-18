//! [`ScopeState`] — the single deterministic projection of a scope's op-log.
//!
//! Same ops, any order, deduped by id → the same `ScopeState` and the same
//! [`ScopeState::root`]. This is the one materializer for a scope: values, the
//! writer/cap ACL, and group membership all live in one state with one root.
//!
//! [`ScopeState::acl_view_at`] resolves the authorization view at an op's
//! causal cut (the ancestry of its parents) — the **causal-honor** view
//! `calimero_authz::authorize` decides against. Convergence comes from
//! per-slot last-writer-wins keyed on `(hlc, op_id)`, so the fold is
//! order-independent.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use sha2::{Digest, Sha256};

use calimero_authz::AclView;
use calimero_context_config::types::ContextGroupId;
use calimero_op::{scope_root, Op, OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::entities::OpMask;
use calimero_storage::logical_clock::HybridTimestamp;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

/// Last-writer-wins stamp for a slot: the `(hlc, op_id)` of the op that last
/// won it. Comparing as a tuple makes concurrent ops (equal `hlc`) tie-break
/// deterministically by content-address `op_id`, so every node picks the same
/// winner regardless of arrival order.
type Stamp = (HybridTimestamp, [u8; 32]);

fn wins(incoming: Stamp, current: Option<&Stamp>) -> bool {
    current.is_none_or(|cur| incoming > *cur)
}

/// Set an LWW register `slot` to `value` iff `stamp` beats the stored stamp.
/// Shares the single comparison rule with [`wins`] so the two cannot diverge.
fn lww_set<T>(slot: &mut Option<(Stamp, T)>, stamp: Stamp, value: T) {
    if wins(stamp, slot.as_ref().map(|(seen, _)| seen)) {
        *slot = Some((stamp, value));
    }
}

/// A subgroup scope's state, each field an independent LWW register keyed on
/// `(hlc, op_id)` so create / reparent / delete converge order-independently:
/// `parent` (set by create + reparent), `restricted` (set by create),
/// `exists` (create → true, delete → false). A scope is live iff its latest
/// `exists` is `true`.
#[derive(Clone, Debug, Default)]
struct SubgroupSlot {
    parent: Option<(Stamp, ScopeId)>,
    restricted: Option<(Stamp, bool)>,
    exists: Option<(Stamp, bool)>,
}

/// The deterministic projection of one scope's op-log: values + ACL + groups,
/// each slot resolved last-writer-wins by `(hlc, op_id)`.
#[derive(Clone, Debug, Default)]
pub struct ScopeState {
    // --- data plane ---
    entities: BTreeMap<Id, Vec<u8>>,
    data_clock: BTreeMap<Id, Stamp>,
    // --- access-control plane ---
    acl: BTreeMap<Id, BTreeMap<PublicKey, OpMask>>,
    acl_clock: BTreeMap<Id, Stamp>,
    // --- membership plane ---
    groups: BTreeMap<ContextGroupId, BTreeMap<PublicKey, GroupMemberRole>>,
    member_clock: BTreeMap<(ContextGroupId, PublicKey), Stamp>,
    // --- admin plane ---
    root_admin: Option<PublicKey>,
    admin_clock: Option<Stamp>,
    policy: Vec<u8>,
    policy_clock: Option<Stamp>,
    subgroups: BTreeMap<ScopeId, SubgroupSlot>,
}

impl ScopeState {
    /// Fold a set of ops into a fresh state. Order-independent (per-slot LWW),
    /// so this is the projection regardless of the order ops arrived.
    #[must_use]
    pub fn from_ops<'a, I: IntoIterator<Item = &'a Op>>(ops: I) -> Self {
        let mut state = Self::default();
        for op in ops {
            state.apply(op);
        }
        state
    }

    /// Apply one op, last-writer-wins per affected slot.
    pub fn apply(&mut self, op: &Op) {
        let stamp: Stamp = (op.hlc, op.id);
        match &op.payload {
            OpPayload::Put { entity, value } => {
                if wins(stamp, self.data_clock.get(entity)) {
                    let _ = self.entities.insert(*entity, value.clone());
                    let _ = self.data_clock.insert(*entity, stamp);
                }
            }
            OpPayload::Delete { entity } => {
                if wins(stamp, self.data_clock.get(entity)) {
                    let _ = self.entities.remove(entity);
                    let _ = self.data_clock.insert(*entity, stamp);
                }
            }
            OpPayload::SetWriters { object, writers } => {
                if wins(stamp, self.acl_clock.get(object)) {
                    let _ = self.acl.insert(*object, writers.clone());
                    let _ = self.acl_clock.insert(*object, stamp);
                }
            }
            OpPayload::MemberAdded {
                group,
                member,
                role,
            } => {
                let key = (*group, *member);
                if wins(stamp, self.member_clock.get(&key)) {
                    let _ = self
                        .groups
                        .entry(*group)
                        .or_default()
                        .insert(*member, role.clone());
                    let _ = self.member_clock.insert(key, stamp);
                }
            }
            OpPayload::MemberRemoved { group, member } => {
                let key = (*group, *member);
                if wins(stamp, self.member_clock.get(&key)) {
                    if let Some(members) = self.groups.get_mut(group) {
                        let _ = members.remove(member);
                        // Drop the group entry once empty so "group never
                        // existed" and "all members removed" are the SAME
                        // materialized state — otherwise a phantom empty map
                        // would perturb `governance_hash` and break
                        // convergence between nodes that reached the empty
                        // group via different op orders. The per-member LWW
                        // bookkeeping in `member_clock` is retained so a later
                        // re-add still has to beat this removal.
                        if members.is_empty() {
                            let _ = self.groups.remove(group);
                        }
                    }
                    let _ = self.member_clock.insert(key, stamp);
                }
            }
            OpPayload::AdminChanged { new_admin } => {
                if wins(stamp, self.admin_clock.as_ref()) {
                    self.root_admin = Some(*new_admin);
                    self.admin_clock = Some(stamp);
                }
            }
            OpPayload::PolicyUpdated { policy_bytes } => {
                if wins(stamp, self.policy_clock.as_ref()) {
                    self.policy.clone_from(policy_bytes);
                    self.policy_clock = Some(stamp);
                }
            }
            OpPayload::SubgroupCreated {
                child,
                parent,
                restricted,
            } => {
                let slot = self.subgroups.entry(*child).or_default();
                lww_set(&mut slot.parent, stamp, *parent);
                lww_set(&mut slot.restricted, stamp, *restricted);
                lww_set(&mut slot.exists, stamp, true);
            }
            OpPayload::SubgroupReparented { child, new_parent } => {
                let slot = self.subgroups.entry(*child).or_default();
                lww_set(&mut slot.parent, stamp, *new_parent);
            }
            OpPayload::SubgroupDeleted { scope } => {
                let slot = self.subgroups.entry(*scope).or_default();
                lww_set(&mut slot.exists, stamp, false);
            }
            // A graph-only node: present in the log so an ancestry walk can
            // traverse through it, but it folds to nothing.
            OpPayload::Noop => {}
        }
    }

    /// The current authorization view (whole state).
    #[must_use]
    pub fn acl_view(&self) -> AclView {
        AclView {
            acl: self.acl.clone(),
            groups: self.groups.clone(),
            root_admin: self.root_admin,
        }
    }

    /// Resolve the [`AclView`] at the causal cut named by `parents` over
    /// `log` — the **causal-honor** view: fold the ops named by `parents` and
    /// their transitive ancestors (the cut is inclusive of `parents`, since an
    /// op's parents causally precede it), never ops causally after the cut. So
    /// a pre-revocation write resolves against the pre-revocation ACL even on a
    /// node that has already applied the revocation (the forward-only property).
    ///
    /// **Precondition (caller's responsibility):** `log` must contain every
    /// ancestor of `parents` within this scope. A parent id not found in `log`
    /// is skipped — correct for a legitimately out-of-slice **cross-scope**
    /// parent edge, but if a *same-scope* ancestor is missing the returned view
    /// is silently computed over a truncated ancestry, which in a security
    /// context could authorize against a stale ACL. The live apply path
    /// guarantees completeness before authorizing: the DAG buffers an op until
    /// all its parents are present, so `authorize` only ever runs on a
    /// fully-materialized ancestry.
    #[must_use]
    pub fn acl_view_at(log: &[Op], parents: &[[u8; 32]]) -> AclView {
        let by_id: HashMap<[u8; 32], &Op> = log.iter().map(|op| (op.id, op)).collect();
        let mut visited: HashSet<[u8; 32]> = HashSet::new();
        let mut queue: VecDeque<[u8; 32]> = parents.iter().copied().collect();
        let mut ancestry: Vec<&Op> = Vec::new();
        while let Some(id) = queue.pop_front() {
            if !visited.insert(id) {
                continue;
            }
            if let Some(op) = by_id.get(&id) {
                ancestry.push(op);
                for parent in &op.parents {
                    queue.push_back(*parent);
                }
            }
        }
        Self::from_ops(ancestry).acl_view()
    }

    /// The single convergence root over the whole projection (values + ACL +
    /// groups). See [`calimero_op::scope_root`].
    #[must_use]
    pub fn root(&self) -> [u8; 32] {
        scope_root(
            self.entities_hash(),
            self.acl_hash(),
            self.governance_hash(),
        )
    }

    /// Compose this scope's convergence root from an **externally supplied**
    /// `entities_root` and this projection's ACL + groups hashes.
    ///
    /// The storage layer keeps its Merkle `root_hash` as `entities_root`, and
    /// the projection folds authorization (writer sets + membership +
    /// admin/policy/subgroups) in on top. Shipping THIS as the wire convergence
    /// signal — instead of the bare entity root — is what makes a hash-neutral
    /// writer/membership rotation (identical entities, different ACL) a
    /// *different* root, so sync can never declare "converged" while
    /// authorization disagrees. The projection never re-hashes entity state;
    /// `entities_root` is authoritative for the data plane.
    ///
    /// **Caller contract:** `entities_root` MUST be the **storage layer's
    /// Merkle root**, not this projection's own [`entities_hash`](Self) — they
    /// are different functions (the storage Merkle vs a SHA-256 over the
    /// projection's `BTreeMap`) and the type system can't tell two `[u8; 32]`s
    /// apart. Passing the projection's own entity hash would produce a
    /// valid-looking root that doesn't carry the intended cross-layer semantics.
    /// This is intentionally NOT [`root`](Self::root) (which uses the
    /// projection's entity hash) — the whole point is to fold authorization onto
    /// the *storage* root.
    #[must_use]
    pub fn scope_root_with_entities(&self, entities_root: [u8; 32]) -> [u8; 32] {
        scope_root(entities_root, self.acl_hash(), self.governance_hash())
    }

    fn entities_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for (id, value) in &self.entities {
            hasher.update(id.as_bytes());
            hasher.update((value.len() as u64).to_le_bytes());
            hasher.update(value);
        }
        hasher.finalize().into()
    }

    fn acl_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for (id, writers) in &self.acl {
            hasher.update(id.as_bytes());
            for (writer, mask) in writers {
                hasher.update(AsRef::<[u8; 32]>::as_ref(writer));
                hasher.update([mask.bits()]);
            }
        }
        hasher.finalize().into()
    }

    /// Hash of the whole **governance** plane: group memberships, the scope's
    /// root admin, its policy, and its live subgroups. Named `governance_hash`
    /// (not `groups_hash`) because it covers admin/policy/subgroups too — it is
    /// the third `scope_root` component (`scope_root(entities, acl,
    /// governance)`); anyone splitting a field out of here must keep
    /// `scope_root` folding all of it in.
    fn governance_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for (group, members) in &self.groups {
            // Skip empty groups so the materialized membership state hashes
            // identically regardless of how it was reached (defensive: `apply`
            // already drops emptied groups). A phantom empty group must never
            // perturb the convergence root.
            if members.is_empty() {
                continue;
            }
            hasher.update(group.to_bytes());
            for (member, role) in members {
                hasher.update(AsRef::<[u8; 32]>::as_ref(member));
                hasher.update([role_byte(role)]);
            }
        }
        if let Some(admin) = &self.root_admin {
            hasher.update([1u8]);
            hasher.update(AsRef::<[u8; 32]>::as_ref(admin));
        } else {
            hasher.update([0u8]);
        }
        hasher.update((self.policy.len() as u64).to_le_bytes());
        hasher.update(&self.policy);
        for (child, slot) in &self.subgroups {
            // Only live subgroups (latest `exists` is true) contribute, with
            // their resolved parent + restricted flag.
            if slot.exists.as_ref().is_some_and(|(_, live)| *live) {
                hasher.update(child.as_bytes());
                if let Some((_, parent)) = &slot.parent {
                    hasher.update(parent.as_bytes());
                }
                hasher.update([u8::from(slot.restricted.as_ref().is_some_and(|(_, r)| *r))]);
            }
        }
        hasher.finalize().into()
    }
}

/// Stable byte for a role in the groups hash. Explicit (not the enum's
/// in-memory discriminant) so the root is invariant across refactors that
/// reorder variants.
fn role_byte(role: &GroupMemberRole) -> u8 {
    match role {
        GroupMemberRole::Admin => 0,
        GroupMemberRole::Member => 1,
        GroupMemberRole::ReadOnly => 2,
        GroupMemberRole::ReadOnlyTee => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_storage::logical_clock::{Timestamp, ID, NTP64};
    use core::num::NonZeroU128;

    fn hlc(ns: u64) -> HybridTimestamp {
        HybridTimestamp::new(Timestamp::new(
            NTP64(ns),
            ID::from(NonZeroU128::new(1).unwrap()),
        ))
    }

    fn op(hlc_ns: u64, payload: OpPayload) -> Op {
        let scope = ScopeId::from([0u8; 32]);
        let author = PublicKey::from([1u8; 32]);
        let h = hlc(hlc_ns);
        let id = Op::compute_id(scope, &[], &author, &h, &payload);
        Op {
            id,
            scope,
            parents: vec![],
            author,
            hlc: h,
            payload,
            expected_scope_root: [0u8; 32],
            signature: [0u8; 64],
        }
    }

    fn sample_ops() -> Vec<Op> {
        let pk = PublicKey::from([9u8; 32]);
        let group = ContextGroupId::from([3u8; 32]);
        vec![
            op(
                10,
                OpPayload::Put {
                    entity: Id::new([1u8; 32]),
                    value: vec![1, 2, 3],
                },
            ),
            op(
                20,
                OpPayload::SetWriters {
                    object: Id::new([2u8; 32]),
                    writers: [(pk, OpMask::FULL)].into_iter().collect(),
                },
            ),
            op(
                30,
                OpPayload::MemberAdded {
                    group,
                    member: pk,
                    role: GroupMemberRole::Member,
                },
            ),
            op(40, OpPayload::AdminChanged { new_admin: pk }),
        ]
    }

    #[test]
    fn projection_is_order_independent() {
        let ops = sample_ops();
        let forward = ScopeState::from_ops(&ops);
        let mut reversed = ops.clone();
        reversed.reverse();
        let backward = ScopeState::from_ops(&reversed);
        assert_eq!(
            forward.root(),
            backward.root(),
            "same ops in any order must converge to the same scope_root"
        );
    }

    #[test]
    fn put_resolves_last_writer_wins_by_hlc() {
        let entity = Id::new([1u8; 32]);
        let older = op(
            10,
            OpPayload::Put {
                entity,
                value: vec![0xAA],
            },
        );
        let newer = op(
            20,
            OpPayload::Put {
                entity,
                value: vec![0xBB],
            },
        );
        // Apply in both orders; the higher-hlc value must win either way.
        let a = ScopeState::from_ops([&older, &newer]);
        let b = ScopeState::from_ops([&newer, &older]);
        assert_eq!(a.root(), b.root());
        assert_eq!(a.entities.get(&entity), Some(&vec![0xBB]));
    }

    #[test]
    fn scope_root_with_entities_detects_hash_neutral_acl_and_membership_changes() {
        // Security property: with the SAME entity root, an ACL or membership
        // change must still move the convergence signal.
        let entities_root = [0x42u8; 32];
        let pk = PublicKey::from([9u8; 32]);
        let group = ContextGroupId::from([3u8; 32]);

        let empty = ScopeState::from_ops::<[&Op; 0]>([]);
        let base = empty.scope_root_with_entities(entities_root);

        // A writer-set rotation (hash-neutral on entities) moves the root.
        let rotated = ScopeState::from_ops([&op(
            10,
            OpPayload::SetWriters {
                object: Id::new([2u8; 32]),
                writers: [(pk, OpMask::FULL)].into_iter().collect(),
            },
        )])
        .scope_root_with_entities(entities_root);
        assert_ne!(
            base, rotated,
            "a writer-set rotation must move scope_root even with identical entities"
        );

        // A membership change (also hash-neutral on entities) moves the root.
        let member_added = ScopeState::from_ops([&op(
            10,
            OpPayload::MemberAdded {
                group,
                member: pk,
                role: GroupMemberRole::Member,
            },
        )])
        .scope_root_with_entities(entities_root);
        assert_ne!(
            base, member_added,
            "a membership change must move scope_root even with identical entities"
        );

        // An admin change moves the root too (governance_hash folds in
        // root_admin), so a silent admin takeover can't pass as converged.
        let admin_changed =
            ScopeState::from_ops([&op(10, OpPayload::AdminChanged { new_admin: pk })])
                .scope_root_with_entities(entities_root);
        assert_ne!(
            base, admin_changed,
            "an admin change must move scope_root even with identical entities"
        );

        // A policy change moves the root (governance_hash folds in policy).
        let policy_changed = ScopeState::from_ops([&op(
            10,
            OpPayload::PolicyUpdated {
                policy_bytes: vec![1, 2, 3],
            },
        )])
        .scope_root_with_entities(entities_root);
        assert_ne!(
            base, policy_changed,
            "a policy change must move scope_root even with identical entities"
        );

        // The entities component still matters: same projection, different
        // storage root ⇒ different scope_root.
        assert_ne!(
            empty.scope_root_with_entities([0x42u8; 32]),
            empty.scope_root_with_entities([0x43u8; 32]),
            "the storage entities root remains authoritative for the data plane"
        );
    }

    #[test]
    fn harness_catches_convergence_and_isolation_over_random_workloads() {
        use std::collections::BTreeSet;

        use crate::testing::{assert_converges_and_isolates, check, simulate, ReplicaView};

        // Three scopes; four replicas with overlapping-but-distinct membership
        // (so every scope has ≥2 members to converge, and ≥1 non-member to
        // isolate from).
        let scopes = [
            ScopeId::from([0xA0; 32]),
            ScopeId::from([0xB0; 32]),
            ScopeId::from([0xC0; 32]),
        ];
        let membership: Vec<BTreeSet<ScopeId>> = vec![
            [scopes[0], scopes[1]].into_iter().collect(),
            [scopes[0], scopes[2]].into_iter().collect(),
            [scopes[0], scopes[1], scopes[2]].into_iter().collect(),
            [scopes[1]].into_iter().collect(),
        ];

        // Seeded op generator: random scope/payload/hlc, distinct ids.
        let mut state = 0x1234_5678_9ABC_DEF0u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let author = PublicKey::from([1u8; 32]);
        let mut ops = Vec::new();
        for _ in 0..120 {
            let scope = scopes[(next() % 3) as usize];
            let h = hlc(next() % 50);
            let entity = Id::new([(next() % 7) as u8; 32]);
            let payload = match next() % 3 {
                0 => OpPayload::Put {
                    entity,
                    value: (next() % 256).to_le_bytes().to_vec(),
                },
                1 => OpPayload::Delete { entity },
                _ => OpPayload::SetWriters {
                    object: entity,
                    writers: [(PublicKey::from([(next() % 5) as u8; 32]), OpMask::FULL)]
                        .into_iter()
                        .collect(),
                },
            };
            let id = Op::compute_id(scope, &[], &author, &h, &payload);
            ops.push(Op {
                id,
                scope,
                parents: vec![],
                author,
                hlc: h,
                payload,
                expected_scope_root: [0u8; 32],
                signature: [0u8; 64],
            });
        }

        // The model must converge + isolate across many delivery orders.
        for seed in 0..16u64 {
            assert_converges_and_isolates(seed, &membership, &ops);
        }

        // Self-check the harness actually *detects* violations: hand a replica
        // a root for a scope it isn't a member of → isolation must fail; and
        // two divergent roots for the same scope → convergence must fail.
        let leak = vec![ReplicaView {
            member_of: BTreeSet::new(),
            roots: [(scopes[0], [7u8; 32])].into_iter().collect(),
        }];
        assert!(
            check(&leak).is_err(),
            "harness must flag a non-member holding a scope root (isolation)"
        );
        let diverge = vec![
            ReplicaView {
                member_of: [scopes[0]].into_iter().collect(),
                roots: [(scopes[0], [1u8; 32])].into_iter().collect(),
            },
            ReplicaView {
                member_of: [scopes[0]].into_iter().collect(),
                roots: [(scopes[0], [2u8; 32])].into_iter().collect(),
            },
        ];
        assert!(
            check(&diverge).is_err(),
            "harness must flag two members disagreeing on a scope root (convergence)"
        );
        // And confirm an honest single replica passes.
        let _ = simulate(0, &membership, &ops);
    }

    #[test]
    fn acl_view_at_honors_the_causal_cut() {
        // genesis: admin adds pk as a writer-set owner.
        let owner = PublicKey::from([9u8; 32]);
        let object = Id::new([2u8; 32]);
        let genesis = op(
            10,
            OpPayload::SetWriters {
                object,
                writers: [(owner, OpMask::FULL)].into_iter().collect(),
            },
        );
        // later: writers cleared (owner removed).
        let mut revoke = op(
            20,
            OpPayload::SetWriters {
                object,
                writers: BTreeMap::new(),
            },
        );
        revoke.parents = vec![genesis.id];

        let log = vec![genesis.clone(), revoke.clone()];

        // View at the cut [genesis] (pre-revoke) still sees the owner.
        let pre = ScopeState::acl_view_at(&log, &[genesis.id]);
        assert!(
            pre.is_owner(&owner, object),
            "pre-revoke cut keeps the owner"
        );

        // View at the cut [revoke] (post) does not.
        let post = ScopeState::acl_view_at(&log, &[revoke.id]);
        assert!(
            !post.is_owner(&owner, object),
            "post-revoke cut drops the owner"
        );
    }

    #[test]
    fn scope_tree_create_reparent_delete_is_order_independent() {
        let child = ScopeId::from([0xC1; 32]);
        let p1 = ScopeId::from([0x11; 32]);
        let p2 = ScopeId::from([0x22; 32]);

        // create(child under p1, restricted)@10 → reparent to p2 @20.
        let ops = vec![
            op(
                10,
                OpPayload::SubgroupCreated {
                    child,
                    parent: p1,
                    restricted: true,
                },
            ),
            op(
                20,
                OpPayload::SubgroupReparented {
                    child,
                    new_parent: p2,
                },
            ),
        ];
        let fwd = ScopeState::from_ops(&ops);
        let mut rev = ops.clone();
        rev.reverse();
        let bwd = ScopeState::from_ops(&rev);
        assert_eq!(
            fwd.root(),
            bwd.root(),
            "create + reparent converge regardless of order (parent LWW = p2)"
        );

        // Deleting the child @30 drops it from the root entirely.
        let mut with_delete = ops;
        with_delete.push(op(30, OpPayload::SubgroupDeleted { scope: child }));
        let deleted_root = ScopeState::from_ops(&with_delete).root();
        assert_ne!(
            deleted_root,
            fwd.root(),
            "a deleted subgroup no longer contributes to scope_root"
        );
    }
}
