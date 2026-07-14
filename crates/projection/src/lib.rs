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

/// Last-writer-wins stamp for a slot: `(hlc, generation, op_id)` of the op that
/// last won it, compared as a tuple.
///
/// - `hlc` orders ops that carry a real wall/logical clock (the data and
///   ACL-rotation planes). It dominates the comparison.
/// - `generation` is the op's causal depth within the cut being resolved
///   (`1 + max(parent generation)`, `0` at a root). It breaks ties when `hlc`
///   is equal — which is the case for the **governance** plane, whose ops all
///   carry `hlc = 0` (the source deltas are stamped with the default clock).
///   Without it, an add → remove → re-add chain on one `(group, member)` slot
///   would tie-break purely by `op_id` (content hash) — causally arbitrary, so
///   a re-add could lose to the earlier remove. Generation makes the causally
///   later op win. Only meaningful inside [`ScopeState::acl_view_at`], which
///   knows each op's ancestry; the streaming `apply` path uses `0`.
/// - `op_id` is the final, content-addressed tie-break for genuinely concurrent
///   ops (equal `hlc` and `generation`), so every node picks the same winner
///   regardless of arrival order.
type Stamp = (HybridTimestamp, u32, [u8; 32]);

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
    // --- capability plane (gates inherited-membership resolution) ---
    default_caps: BTreeMap<ContextGroupId, u32>,
    default_caps_clock: BTreeMap<ContextGroupId, Stamp>,
    member_caps: BTreeMap<(ContextGroupId, PublicKey), u32>,
    member_caps_clock: BTreeMap<(ContextGroupId, PublicKey), Stamp>,
    // --- per-group admin (the subgroup creator / genesis admin) ---
    group_admin: BTreeMap<ContextGroupId, PublicKey>,
    group_admin_clock: BTreeMap<ContextGroupId, Stamp>,
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

    /// Apply one op, last-writer-wins per affected slot. Streaming entry point
    /// (no ancestry context), so it uses causal generation `0`; the cut-aware
    /// [`Self::acl_view_at`] supplies real generations.
    ///
    /// # The maintained projection is convergent but NOT causally authoritative
    ///
    /// Because this path stamps every op with generation `0`, the **governance**
    /// plane — whose ops all carry `hlc = 0` — tie-breaks purely by `op_id`. An
    /// add → remove → re-add chain on one `(group, member)` slot therefore
    /// resolves by content hash, not by causal order, so a re-add can lose to the
    /// earlier remove. The result is still **deterministic and order-independent**
    /// (every node folds the same ops to the same state, so a projection built
    /// this way — e.g. behind `scope_root_for` — converges across nodes and is
    /// safe to use as a sync convergence signal). It is simply not the
    /// *causally-correct* membership view: it can encode a member as absent while
    /// [`Self::acl_view_at`] (which walks each op's ancestry and assigns real
    /// generations) resolves them present.
    ///
    /// So: use the maintained projection for convergence, and use
    /// [`Self::acl_view_at`] — never this streaming fold — as the authoritative
    /// answer to "is this identity a member at this causal cut" for
    /// authorization.
    pub fn apply(&mut self, op: &Op) {
        self.apply_with_generation(op, 0);
    }

    /// Apply one op with an explicit causal `generation` for its LWW stamp.
    pub fn apply_with_generation(&mut self, op: &Op, generation: u32) {
        let stamp: Stamp = (op.hlc, generation, op.id());
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
                admin,
            } => {
                let slot = self.subgroups.entry(*child).or_default();
                lww_set(&mut slot.parent, stamp, *parent);
                lww_set(&mut slot.restricted, stamp, *restricted);
                lww_set(&mut slot.exists, stamp, true);
                // The creator is the subgroup's genesis admin.
                let g = ContextGroupId::from(*child.as_bytes());
                if wins(stamp, self.group_admin_clock.get(&g)) {
                    let _ = self.group_admin.insert(g, *admin);
                    let _ = self.group_admin_clock.insert(g, stamp);
                }
            }
            OpPayload::SubgroupReparented { child, new_parent } => {
                let slot = self.subgroups.entry(*child).or_default();
                lww_set(&mut slot.parent, stamp, *new_parent);
                // Reparenting a subgroup asserts it exists: an op that reparents
                // it causally follows its creation. Without this, a reparent that
                // arrives (or folds) before the create op leaves `exists` unset,
                // so the subgroup is transiently hidden from `acl_view`/
                // `governance_hash` even though it is live. A later delete still
                // wins by its higher stamp, so this only fills the create gap.
                lww_set(&mut slot.exists, stamp, true);
            }
            OpPayload::SubgroupDeleted { scope } => {
                let slot = self.subgroups.entry(*scope).or_default();
                lww_set(&mut slot.exists, stamp, false);
            }
            OpPayload::SubgroupVisibilitySet { scope, restricted } => {
                let slot = self.subgroups.entry(*scope).or_default();
                lww_set(&mut slot.restricted, stamp, *restricted);
                // Setting visibility likewise asserts existence (see reparent):
                // it's a mutation of a live subgroup, so it must not leave the
                // slot non-existent when it folds before the create.
                lww_set(&mut slot.exists, stamp, true);
            }
            OpPayload::DefaultCapabilitiesSet {
                group,
                capabilities,
            } => {
                if wins(stamp, self.default_caps_clock.get(group)) {
                    let _ = self.default_caps.insert(*group, capabilities.bits());
                    let _ = self.default_caps_clock.insert(*group, stamp);
                }
            }
            OpPayload::MemberCapabilitySet {
                group,
                member,
                capabilities,
            } => {
                let key = (*group, *member);
                if wins(stamp, self.member_caps_clock.get(&key)) {
                    let _ = self.member_caps.insert(key, capabilities.bits());
                    let _ = self.member_caps_clock.insert(key, stamp);
                }
            }
            // A graph-only node: present in the log so an ancestry walk can
            // traverse through it, but it folds to nothing.
            OpPayload::Noop => {}
        }
    }

    /// The current authorization view (whole state).
    #[must_use]
    pub fn acl_view(&self) -> AclView {
        let subgroups = self
            .subgroups
            .iter()
            .filter(|(_, slot)| slot.exists.as_ref().is_some_and(|(_, live)| *live))
            .filter_map(|(child, slot)| {
                slot.parent.as_ref().map(|(_, parent)| {
                    (
                        *child,
                        calimero_authz::SubgroupEdge {
                            parent: *parent,
                            restricted: slot.restricted.as_ref().is_some_and(|(_, r)| *r),
                        },
                    )
                })
            })
            .collect();
        AclView {
            acl: self.acl.clone(),
            groups: self.groups.clone(),
            root_admin: self.root_admin,
            default_caps: self.default_caps.clone(),
            member_caps: self.member_caps.clone(),
            subgroups,
            group_admin: self.group_admin.clone(),
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
        let by_id: HashMap<[u8; 32], &Op> = log.iter().map(|op| (op.id(), op)).collect();
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

        // Causal generation per ancestry op = longest path from a root
        // (`1 + max(parent generation)`, `0` when no parent is in the ancestry).
        // It orders causally-related governance ops (whose `hlc` is all `0`) so a
        // re-add beats an earlier remove. Iterative post-order over the DAG (no
        // cycles), so deep chains can't blow the stack.
        let anc_by_id: HashMap<[u8; 32], &Op> = ancestry.iter().map(|op| (op.id(), *op)).collect();
        let mut generation: HashMap<[u8; 32], u32> = HashMap::new();
        for &start in &ancestry {
            if generation.contains_key(&start.id()) {
                continue;
            }
            let mut stack = vec![start.id()];
            while let Some(&top) = stack.last() {
                let Some(top_op) = anc_by_id.get(&top) else {
                    let _ = generation.insert(top, 0);
                    let _ = stack.pop();
                    continue;
                };
                let parents_in_anc: Vec<[u8; 32]> = top_op
                    .parents
                    .iter()
                    .copied()
                    .filter(|p| anc_by_id.contains_key(p))
                    .collect();
                let unresolved: Vec<[u8; 32]> = parents_in_anc
                    .iter()
                    .copied()
                    .filter(|p| !generation.contains_key(p))
                    .collect();
                if unresolved.is_empty() {
                    let g = parents_in_anc
                        .iter()
                        .map(|p| generation[p] + 1)
                        .max()
                        .unwrap_or(0);
                    let _ = generation.insert(top, g);
                    let _ = stack.pop();
                } else {
                    stack.extend(unresolved);
                }
            }
        }

        let mut state = Self::default();
        for &op in &ancestry {
            state.apply_with_generation(op, generation.get(&op.id()).copied().unwrap_or(0));
        }
        state.acl_view()
    }

    /// Is the **complete** causal ancestry of `parents` present in `log` — i.e.
    /// does the walk reach every referenced op without truncating at a missing
    /// one? `acl_view_at` silently skips a missing ancestor (correct for a
    /// legitimately out-of-slice cross-scope edge, but a *same-scope* gap yields
    /// a truncated, possibly-stale view). The **authoritative grant** path must
    /// not override live's reject on a truncated view: a missing mid-ancestry
    /// removal would leave a since-removed member still folded as present. This
    /// returns `false` the moment any referenced id is absent from `log`, so the
    /// grant can abstain (defer to live) unless it has the whole history.
    #[must_use]
    pub fn cut_ancestry_complete(log: &[Op], parents: &[[u8; 32]]) -> bool {
        let by_id: HashMap<[u8; 32], &Op> = log.iter().map(|op| (op.id(), op)).collect();
        let mut visited: HashSet<[u8; 32]> = HashSet::new();
        let mut queue: VecDeque<[u8; 32]> = parents.iter().copied().collect();
        while let Some(id) = queue.pop_front() {
            if !visited.insert(id) {
                continue;
            }
            match by_id.get(&id) {
                Some(op) => queue.extend(op.parents.iter().copied()),
                None => return false,
            }
        }
        true
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
        // Capability plane (deterministic order via BTreeMap). Folded into the
        // convergence root so a node's caps view can't silently diverge.
        for (group, caps) in &self.default_caps {
            hasher.update(group.to_bytes());
            hasher.update(caps.to_le_bytes());
        }
        for ((group, member), caps) in &self.member_caps {
            hasher.update(group.to_bytes());
            hasher.update(AsRef::<[u8; 32]>::as_ref(member));
            hasher.update(caps.to_le_bytes());
        }
        for (group, admin) in &self.group_admin {
            hasher.update(group.to_bytes());
            hasher.update(AsRef::<[u8; 32]>::as_ref(admin));
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
        Op::new(scope, vec![], author, h, payload, [0u8; 32], [0u8; 64])
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
            ops.push(Op::new(
                scope,
                vec![],
                author,
                h,
                payload,
                [0u8; 32],
                [0u8; 64],
            ));
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
        revoke.parents = vec![genesis.id()];

        let log = vec![genesis.clone(), revoke.clone()];

        // View at the cut [genesis] (pre-revoke) still sees the owner.
        let pre = ScopeState::acl_view_at(&log, &[genesis.id()]);
        assert!(
            pre.is_owner(&owner, object),
            "pre-revoke cut keeps the owner"
        );

        // View at the cut [revoke] (post) does not.
        let post = ScopeState::acl_view_at(&log, &[revoke.id()]);
        assert!(
            !post.is_owner(&owner, object),
            "post-revoke cut drops the owner"
        );
    }

    #[test]
    fn re_add_after_remove_wins_at_zero_hlc_via_causal_generation() {
        // Governance ops all carry hlc=0, so only causal generation orders them.
        // An add → remove → re-add chain on one (group, member) slot must resolve
        // to PRESENT at the re-add cut, not lose to the remove by op_id tie-break
        // (the kick-and-readd bug).
        let group = ContextGroupId::from([3u8; 32]);
        let member = PublicKey::from([0x55; 32]);
        let scope = ScopeId::from([0u8; 32]);
        let author = PublicKey::from([1u8; 32]);
        let zero = hlc(0);
        let mk = |parents: Vec<[u8; 32]>, payload: OpPayload| -> Op {
            Op::new(scope, parents, author, zero, payload, [0u8; 32], [0u8; 64])
        };
        let add = mk(
            vec![],
            OpPayload::MemberAdded {
                group,
                member,
                role: GroupMemberRole::Member,
            },
        );
        let remove = mk(vec![add.id()], OpPayload::MemberRemoved { group, member });
        let readd = mk(
            vec![remove.id()],
            OpPayload::MemberAdded {
                group,
                member,
                role: GroupMemberRole::Admin,
            },
        );
        let log = vec![add.clone(), remove.clone(), readd.clone()];

        // At the re-add cut: present as Admin (generation readd > remove > add).
        let at_readd = ScopeState::acl_view_at(&log, &[readd.id()]);
        assert_eq!(
            at_readd.groups.get(&group).and_then(|m| m.get(&member)),
            Some(&GroupMemberRole::Admin),
            "re-add after remove must win by causal generation at hlc=0"
        );

        // At the remove cut: absent.
        let at_remove = ScopeState::acl_view_at(&log, &[remove.id()]);
        assert_eq!(
            at_remove.groups.get(&group).and_then(|m| m.get(&member)),
            None,
            "the remove cut drops the member"
        );
    }

    #[test]
    fn cut_ancestry_complete_detects_a_missing_mid_ancestry_op() {
        // add → remove chain; the cut cites `remove`. With both folded the
        // ancestry is complete; drop the mid-ancestry `add` (parent of `remove`)
        // and the walk truncates → incomplete. This is the over-grant guard: a
        // missing removal in the middle must NOT pass as an authoritative grant.
        let group = ContextGroupId::from([3u8; 32]);
        let member = PublicKey::from([0x55; 32]);
        let scope = ScopeId::from([0u8; 32]);
        let author = PublicKey::from([1u8; 32]);
        let zero = hlc(0);
        let mk = |parents: Vec<[u8; 32]>, payload: OpPayload| -> Op {
            Op::new(scope, parents, author, zero, payload, [0u8; 32], [0u8; 64])
        };
        let add = mk(
            vec![],
            OpPayload::MemberAdded {
                group,
                member,
                role: GroupMemberRole::Member,
            },
        );
        let remove = mk(vec![add.id()], OpPayload::MemberRemoved { group, member });

        // Complete log: ancestry of [remove] = {remove, add}, both present.
        let complete = vec![add.clone(), remove.clone()];
        assert!(ScopeState::cut_ancestry_complete(&complete, &[remove.id()]));

        // Missing the mid-ancestry `add` (remove's parent) → truncated → false.
        let truncated = vec![remove.clone()];
        assert!(!ScopeState::cut_ancestry_complete(
            &truncated,
            &[remove.id()]
        ));

        // A cited head absent from the log → also incomplete.
        assert!(!ScopeState::cut_ancestry_complete(&[], &[remove.id()]));
    }

    #[test]
    fn reparent_or_visibility_before_create_does_not_hide_subgroup() {
        let child = ScopeId::from([0xC1; 32]);
        let p2 = ScopeId::from([0x22; 32]);

        // Only a reparent has folded so far (the create hasn't arrived yet): the
        // subgroup must still resolve as live, not be transiently hidden.
        let reparent = op(
            20,
            OpPayload::SubgroupReparented {
                child,
                new_parent: p2,
            },
        );
        assert!(
            ScopeState::from_ops([&reparent])
                .acl_view()
                .subgroups
                .contains_key(&child),
            "a reparent must assert the subgroup exists even before the create folds"
        );

        // Same for a visibility change arriving before the create. A visibility
        // op carries no parent, so a parent-less subgroup never appears in
        // `acl_view` (a tree node needs a parent edge); the existence assertion
        // instead shows up as the subgroup now folding into the governance root.
        let vis = op(
            20,
            OpPayload::SubgroupVisibilitySet {
                scope: child,
                restricted: true,
            },
        );
        assert_ne!(
            ScopeState::from_ops([&vis]).root(),
            ScopeState::default().root(),
            "a visibility set must assert the subgroup exists (folds into the root)"
        );

        // A later delete still wins by its higher stamp — the assertion only
        // fills the create gap, it does not resurrect a deleted subgroup.
        let del = op(30, OpPayload::SubgroupDeleted { scope: child });
        assert!(
            !ScopeState::from_ops([&reparent, &del])
                .acl_view()
                .subgroups
                .contains_key(&child),
            "a later delete must still remove the subgroup"
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
                    admin: PublicKey::from([7u8; 32]),
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
