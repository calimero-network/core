//! [`ScopeState`] — the single deterministic projection of a scope's op-log
//! (core#2716, Phase 5).
//!
//! Same ops, any order, deduped by id → the same `ScopeState` and the same
//! [`ScopeState::root`]. This is the one materializer that replaces the
//! parallel storage-apply and governance-apply folds: values, the writer/cap
//! ACL, and group membership all live in one state with one root.
//!
//! [`ScopeState::acl_view_at`] resolves the authorization view at an op's
//! causal cut (the ancestry of its parents) — the **causal-honor** view
//! `calimero_authz::authorize` decides against. Convergence comes from
//! per-slot last-writer-wins keyed on `(hlc, op_id)`, so the fold is
//! order-independent.
//!
//! Additive scaffolding — the live storage/governance apply paths are migrated
//! onto this in a later Phase-5 stage (gated behind the isolation harness).

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

/// Last-writer-wins stamp for a slot: the `(hlc, op_id)` of the op that last
/// won it. Comparing as a tuple makes concurrent ops (equal `hlc`) tie-break
/// deterministically by content-address `op_id`, so every node picks the same
/// winner regardless of arrival order.
type Stamp = (HybridTimestamp, [u8; 32]);

fn wins(incoming: Stamp, current: Option<&Stamp>) -> bool {
    current.is_none_or(|cur| incoming > *cur)
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
    subgroups: BTreeMap<ScopeId, bool>,
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
            OpPayload::SubgroupCreated { child, restricted } => {
                // Create-only and idempotent — no LWW slot needed.
                let _ = self.subgroups.insert(*child, *restricted);
            }
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
    /// `log` — the **causal-honor** view: fold only the ops in the ancestry of
    /// `parents` (transitive closure of `parents`), never ops causally after
    /// the cut. So a pre-revocation write resolves against the pre-revocation
    /// ACL even on a node that has already applied the revocation — the
    /// forward-only property P4's `acl_view_at` provided, generalized to the
    /// whole projection.
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
        scope_root(self.entities_hash(), self.acl_hash(), self.groups_hash())
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

    fn groups_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for (group, members) in &self.groups {
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
        for (child, restricted) in &self.subgroups {
            hasher.update(child.as_bytes());
            hasher.update([u8::from(*restricted)]);
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
}
