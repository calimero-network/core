//! Ops that carry no authority because their signer was removed concurrently.
//! The void set depends only on the set of ops, never on their arrival order.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::rc::Rc;

use calimero_account::{AccountId, DeviceId};
use calimero_authz::AclView;
use calimero_context_config::types::ContextGroupId;
use calimero_op::{Authorship, Op, OpPayload};
use calimero_primitives::context::GroupMemberRole;

use crate::ScopeState;

/// Rounds of the search before its tail is treated as a cycle.
const MAX_ROUNDS: usize = 32;

/// Ops folded judging cascades; past it the remaining candidates are void.
/// Their signers' standing rests on a void op, and every node must agree.
const MAX_FOLD_WORK: usize = 50_000_000;

/// Facts the authority check reads that no op carries, written by the store at genesis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AuthorityBase {
    /// The namespace root group and its genesis admin.
    pub root: Option<(ContextGroupId, AccountId)>,
    /// The root group's default member capabilities.
    pub default_cap_base: u32,
}

type IdSet = Rc<HashSet<[u8; 32]>>;

#[derive(Clone, Copy)]
enum Target {
    Account(AccountId),
    Device(DeviceId),
}

/// A removal, demotion or device revocation, and what it takes away.
#[derive(Clone, Copy)]
struct Removal<'a> {
    op: &'a Op,
    target: Target,
    /// Where the target loses standing; `None` for a device, everywhere.
    group: Option<ContextGroupId>,
}

impl ScopeState {
    /// The ops of `log` that carry no authority.
    #[must_use]
    pub fn void_ops(log: &[Op], base: AuthorityBase) -> BTreeSet<[u8; 32]> {
        Self::void_ops_with(log, base, None)
    }

    /// [`Self::void_ops`] over `log` plus `candidate`, an op about to join it.
    /// `group` names where it acts when its payload does not; its cut must be whole.
    #[must_use]
    pub fn void_ops_with(
        log: &[Op],
        base: AuthorityBase,
        candidate: Option<(&Op, Option<ContextGroupId>)>,
    ) -> BTreeSet<[u8; 32]> {
        Self::void_ops_judging(log, base, candidate, &[])
    }

    /// [`Self::void_ops_with`] that also judges `held`, ops of `log` the projection
    /// models nothing about, each with the group it acted in.
    #[must_use]
    pub fn void_ops_judging(
        log: &[Op],
        base: AuthorityBase,
        candidate: Option<(&Op, Option<ContextGroupId>)>,
        held: &[([u8; 32], ContextGroupId)],
    ) -> BTreeSet<[u8; 32]> {
        Self::void_ops_bounded(log, base, candidate, held, MAX_FOLD_WORK)
    }

    /// [`Self::void_ops_judging`] with an explicit bound on the ops folded.
    pub(crate) fn void_ops_bounded(
        log: &[Op],
        base: AuthorityBase,
        candidate: Option<(&Op, Option<ContextGroupId>)>,
        held: &[([u8; 32], ContextGroupId)],
        budget: usize,
    ) -> BTreeSet<[u8; 32]> {
        let mut ops: Vec<&Op> = log.iter().collect();
        let mut explicit: HashMap<[u8; 32], ContextGroupId> = held.iter().copied().collect();
        if let Some((op, group)) = candidate {
            if let Some(group) = group {
                let _ = explicit.insert(op.id(), group);
            }
            if !log.iter().any(|held| held.id() == op.id()) {
                ops.push(op);
            }
        }
        // Most logs hold no removal that could matter; say so before building anything.
        if !holds_removal(&ops) {
            return BTreeSet::new();
        }
        Analysis::new(&ops, base, explicit, budget).run()
    }
}

/// The group `op` acts in, for payloads whose authority a removal can take away.
fn payload_group(op: &Op) -> Option<ContextGroupId> {
    let root = || ContextGroupId::from(*op.scope.as_bytes());
    match &op.payload {
        OpPayload::MemberAdded { group, .. }
        | OpPayload::MemberRemoved { group, .. }
        | OpPayload::MemberCapabilitySet { group, .. }
        | OpPayload::DefaultCapabilitiesSet { group, .. } => Some(*group),
        OpPayload::SubgroupVisibilitySet { scope, .. } => {
            Some(ContextGroupId::from(*scope.as_bytes()))
        }
        OpPayload::SubgroupCreated { parent, .. } => Some(ContextGroupId::from(*parent.as_bytes())),
        OpPayload::SubgroupReparented { .. }
        | OpPayload::SubgroupDeleted { .. }
        | OpPayload::AdminChanged { .. }
        | OpPayload::PolicyUpdated { .. } => Some(root()),
        _ => None,
    }
}

/// The account `op` would be removing, for the mutual-removal exemption.
fn removes_account(op: &Op) -> Option<AccountId> {
    match &op.payload {
        OpPayload::MemberRemoved { member, .. } => Some(*member),
        OpPayload::MemberAdded { member, role, .. } if !matches!(role, GroupMemberRole::Admin) => {
            Some(*member)
        }
        _ => None,
    }
}

/// Does `ops` hold an op that takes authority away from someone else: a member
/// removal, a device revocation, or a role change for an account granted `Admin`?
fn holds_removal(ops: &[&Op]) -> bool {
    let mut admin_grants: HashSet<(ContextGroupId, AccountId)> = HashSet::new();
    let mut role_changes: Vec<(ContextGroupId, AccountId)> = Vec::new();
    for op in ops {
        if op.author() == Authorship::UNATTRIBUTED_ACCOUNT {
            continue;
        }
        match &op.payload {
            OpPayload::MemberRemoved { member, .. } if op.author() != *member => return true,
            OpPayload::DeviceRevoked { device, .. } if op.device() != *device => return true,
            OpPayload::MemberAdded {
                group,
                member,
                role: GroupMemberRole::Admin,
            }
            | OpPayload::MemberJoinedWithDevice {
                group,
                member,
                role: GroupMemberRole::Admin,
                ..
            } => {
                let _ = admin_grants.insert((*group, *member));
            }
            OpPayload::SubgroupCreated { child, admin, .. } => {
                let _ = admin_grants.insert((ContextGroupId::from(*child.as_bytes()), *admin));
            }
            OpPayload::MemberAdded { group, member, .. } if op.author() != *member => {
                role_changes.push((*group, *member));
            }
            _ => {}
        }
    }
    role_changes
        .iter()
        .any(|change| admin_grants.contains(change))
}

/// The account and device of each op built without an author, from the log's device
/// links (a revocation drops the binding, not the link). A key linked twice names none.
fn resolve_unattributed(ops: &[&Op]) -> HashMap<[u8; 32], (AccountId, DeviceId)> {
    let mut any = false;
    let mut linked: HashMap<[u8; 32], Option<(AccountId, DeviceId)>> = HashMap::new();
    for op in ops {
        if op.author() == Authorship::UNATTRIBUTED_ACCOUNT {
            any = true;
        }
        let (OpPayload::DeviceLinked { cert, .. } | OpPayload::MemberJoinedWithDevice { cert, .. }) =
            &op.payload
        else {
            continue;
        };
        let bound = (cert.account, cert.device);
        let key: [u8; 32] = *cert.sign_pk.as_ref();
        let _ = linked
            .entry(key)
            .and_modify(|held| {
                if *held != Some(bound) {
                    *held = None;
                }
            })
            .or_insert(Some(bound));
    }
    if !any {
        return HashMap::new();
    }
    ops.iter()
        .filter(|op| op.author() == Authorship::UNATTRIBUTED_ACCOUNT)
        .filter_map(|op| {
            let key: &[u8; 32] = op.device_key().as_ref();
            let bound = (*linked.get(key)?)?;
            Some((op.id(), bound))
        })
        .collect()
}

struct Analysis<'a> {
    base: AuthorityBase,
    budget: usize,
    /// The account and device of each op built without an author, as the log's own
    /// device links name them.
    resolved: HashMap<[u8; 32], (AccountId, DeviceId)>,
    by_id: HashMap<[u8; 32], &'a Op>,
    /// The candidate's group, when its payload names none.
    explicit: HashMap<[u8; 32], ContextGroupId>,
    /// Removals that need no judgement: a member removal or a device revocation.
    removals: Vec<Removal<'a>>,
    /// Role changes to a non-admin role for an account that was granted `Admin`;
    /// a demotion only when the account was an admin at the change's cut.
    demotion_candidates: Vec<(&'a Op, ContextGroupId, AccountId)>,
    /// Voidable ops by author and by device.
    by_account: HashMap<AccountId, Vec<&'a Op>>,
    by_device: HashMap<DeviceId, Vec<&'a Op>>,
    depth: HashMap<[u8; 32], u32>,
    children: HashMap<[u8; 32], Vec<[u8; 32]>>,
    parent_group: BTreeMap<ContextGroupId, ContextGroupId>,
    ancestors: RefCell<HashMap<[u8; 32], IdSet>>,
    descendants: RefCell<HashMap<[u8; 32], IdSet>>,
    work: Cell<usize>,
}

impl<'a> Analysis<'a> {
    fn new(
        ops: &[&'a Op],
        base: AuthorityBase,
        explicit: HashMap<[u8; 32], ContextGroupId>,
        budget: usize,
    ) -> Self {
        let by_id: HashMap<[u8; 32], &Op> = ops.iter().map(|op| (op.id(), *op)).collect();

        let mut admin_grants: HashSet<(ContextGroupId, AccountId)> = HashSet::new();
        for op in ops {
            match &op.payload {
                OpPayload::MemberAdded {
                    group,
                    member,
                    role: GroupMemberRole::Admin,
                }
                | OpPayload::MemberJoinedWithDevice {
                    group,
                    member,
                    role: GroupMemberRole::Admin,
                    ..
                } => {
                    let _ = admin_grants.insert((*group, *member));
                }
                OpPayload::SubgroupCreated { child, admin, .. } => {
                    let _ = admin_grants.insert((ContextGroupId::from(*child.as_bytes()), *admin));
                }
                _ => {}
            }
        }

        let resolved = resolve_unattributed(ops);
        let author = |op: &Op| {
            resolved
                .get(&op.id())
                .map_or(op.author(), |(account, _)| *account)
        };
        let device = |op: &Op| {
            resolved
                .get(&op.id())
                .map_or(op.device(), |(_, device)| *device)
        };
        let attributed = |op: &Op| author(op) != Authorship::UNATTRIBUTED_ACCOUNT;
        let mut removals = Vec::new();
        let mut demotion_candidates = Vec::new();
        for &op in ops {
            if !attributed(op) {
                continue;
            }
            match &op.payload {
                OpPayload::MemberRemoved { group, member } if author(op) != *member => {
                    removals.push(Removal {
                        op,
                        target: Target::Account(*member),
                        group: Some(*group),
                    });
                }
                OpPayload::DeviceRevoked { device: target, .. } if device(op) != *target => {
                    removals.push(Removal {
                        op,
                        target: Target::Device(*target),
                        group: None,
                    });
                }
                OpPayload::MemberAdded {
                    group,
                    member,
                    role,
                } if !matches!(role, GroupMemberRole::Admin)
                    && author(op) != *member
                    && admin_grants.contains(&(*group, *member)) =>
                {
                    demotion_candidates.push((op, *group, *member));
                }
                _ => {}
            }
        }

        let mut analysis = Self {
            base,
            budget,
            resolved,
            by_id,
            explicit,
            removals,
            demotion_candidates,
            by_account: HashMap::new(),
            by_device: HashMap::new(),
            depth: HashMap::new(),
            children: HashMap::new(),
            parent_group: BTreeMap::new(),
            ancestors: RefCell::default(),
            descendants: RefCell::default(),
            work: Cell::new(0),
        };
        if analysis.removals.is_empty() && analysis.demotion_candidates.is_empty() {
            return analysis;
        }
        for &op in ops {
            let (account, device) = (analysis.author_of(op), analysis.device_of(op));
            if account != Authorship::UNATTRIBUTED_ACCOUNT && analysis.voidable_group(op).is_some()
            {
                analysis.by_account.entry(account).or_default().push(op);
                analysis.by_device.entry(device).or_default().push(op);
            }
            for parent in &op.parents {
                if analysis.by_id.contains_key(parent) {
                    analysis.children.entry(*parent).or_default().push(op.id());
                }
            }
        }
        analysis.depth = analysis.depths(ops);
        analysis.parent_group = analysis.group_tree(ops);
        analysis
    }

    /// The account `op` speaks for, however the node that built it saw the signer.
    fn author_of(&self, op: &Op) -> AccountId {
        self.resolved
            .get(&op.id())
            .map_or(op.author(), |(account, _)| *account)
    }

    /// The device `op` was signed with.
    fn device_of(&self, op: &Op) -> DeviceId {
        self.resolved
            .get(&op.id())
            .map_or(op.device(), |(_, device)| *device)
    }

    /// The group `op` acts in, if its authority can be voided.
    fn voidable_group(&self, op: &Op) -> Option<ContextGroupId> {
        payload_group(op).or_else(|| self.explicit.get(&op.id()).copied())
    }

    /// Longest path from a root, through the ops the log holds.
    fn depths(&self, ops: &[&Op]) -> HashMap<[u8; 32], u32> {
        let mut depth: HashMap<[u8; 32], u32> = HashMap::new();
        for &start in ops {
            let mut stack = vec![start.id()];
            while let Some(&top) = stack.last() {
                if depth.contains_key(&top) {
                    let _ = stack.pop();
                    continue;
                }
                let op = self.by_id[&top];
                let held: Vec<[u8; 32]> = op
                    .parents
                    .iter()
                    .copied()
                    .filter(|p| self.by_id.contains_key(p))
                    .collect();
                let unresolved: Vec<[u8; 32]> = held
                    .iter()
                    .copied()
                    .filter(|p| !depth.contains_key(p))
                    .collect();
                if unresolved.is_empty() {
                    let here = held.iter().map(|p| depth[p] + 1).max().unwrap_or(0);
                    let _ = depth.insert(top, here);
                    let _ = stack.pop();
                } else {
                    stack.extend(unresolved);
                }
            }
        }
        depth
    }

    /// Each subgroup's parent, the latest creation or reparent winning as the
    /// fold does.
    fn group_tree(&self, ops: &[&Op]) -> BTreeMap<ContextGroupId, ContextGroupId> {
        let mut latest: BTreeMap<ContextGroupId, ((u32, [u8; 32]), ContextGroupId)> =
            BTreeMap::new();
        for &op in ops {
            let (child, parent) = match &op.payload {
                OpPayload::SubgroupCreated { child, parent, .. } => (child, parent),
                OpPayload::SubgroupReparented { child, new_parent } => (child, new_parent),
                _ => continue,
            };
            let stamp = (self.depth.get(&op.id()).copied().unwrap_or(0), op.id());
            let child = ContextGroupId::from(*child.as_bytes());
            if latest.get(&child).is_none_or(|(held, _)| stamp > *held) {
                let _ = latest.insert(child, (stamp, ContextGroupId::from(*parent.as_bytes())));
            }
        }
        latest
            .into_iter()
            .map(|(child, (_, parent))| (child, parent))
            .collect()
    }

    /// `group` and the groups above it.
    fn group_and_ancestors(&self, group: ContextGroupId) -> Vec<ContextGroupId> {
        let mut chain = vec![group];
        let mut at = group;
        while let Some(parent) = self.parent_group.get(&at) {
            if chain.contains(parent) || chain.len() > 64 {
                break;
            }
            chain.push(*parent);
            at = *parent;
        }
        chain
    }

    fn ancestors_of(&self, id: [u8; 32]) -> IdSet {
        if let Some(held) = self.ancestors.borrow().get(&id) {
            return Rc::clone(held);
        }
        let mut seen: HashSet<[u8; 32]> = HashSet::new();
        let mut queue: VecDeque<[u8; 32]> = self.by_id[&id].parents.iter().copied().collect();
        while let Some(next) = queue.pop_front() {
            if !seen.insert(next) {
                continue;
            }
            if let Some(op) = self.by_id.get(&next) {
                queue.extend(op.parents.iter().copied());
            }
        }
        let seen = Rc::new(seen);
        let _ = self.ancestors.borrow_mut().insert(id, Rc::clone(&seen));
        seen
    }

    fn descendants_of(&self, id: [u8; 32]) -> IdSet {
        if let Some(held) = self.descendants.borrow().get(&id) {
            return Rc::clone(held);
        }
        let mut seen: HashSet<[u8; 32]> = HashSet::new();
        let mut queue: VecDeque<[u8; 32]> = self
            .children
            .get(&id)
            .map(|kids| kids.iter().copied().collect())
            .unwrap_or_default();
        while let Some(next) = queue.pop_front() {
            if !seen.insert(next) {
                continue;
            }
            if let Some(kids) = self.children.get(&next) {
                queue.extend(kids.iter().copied());
            }
        }
        let seen = Rc::new(seen);
        let _ = self.descendants.borrow_mut().insert(id, Rc::clone(&seen));
        seen
    }

    /// Fold `ops` at their causal depth.
    fn fold<'o>(&self, ops: impl Iterator<Item = &'o Op>) -> AclView {
        let mut state = ScopeState::default();
        for op in ops {
            self.work.set(self.work.get() + 1);
            state.apply_with_generation(op, self.depth.get(&op.id()).copied().unwrap_or(0));
        }
        state.acl_view()
    }

    fn over_budget(&self) -> bool {
        self.work.get() >= self.budget
    }

    fn run(&self) -> BTreeSet<[u8; 32]> {
        let mut history: Vec<BTreeSet<[u8; 32]>> = vec![BTreeSet::new()];
        loop {
            let current = &history[history.len() - 1];
            let next = self.round(current);
            if next == *current {
                return next;
            }
            let cycle_from = history
                .iter()
                .position(|seen| *seen == next)
                .or_else(|| (history.len() >= MAX_ROUNDS).then_some(history.len() / 2));
            if let Some(from) = cycle_from {
                let mut union = next;
                for seen in &history[from..] {
                    union.extend(seen.iter().copied());
                }
                return union;
            }
            history.push(next);
        }
    }

    /// One application of the rule to the void set `void`.
    fn round(&self, void: &BTreeSet<[u8; 32]>) -> BTreeSet<[u8; 32]> {
        let mut next = BTreeSet::new();
        let owner = self.base.root.map(|(_, admin)| admin);

        for removal in self.effective_removals(void) {
            let before = self.ancestors_of(removal.op.id());
            let after = self.descendants_of(removal.op.id());
            let victims: &[&Op] = match removal.target {
                Target::Account(account) => self.by_account.get(&account),
                Target::Device(device) => self.by_device.get(&device),
            }
            .map_or(&[], Vec::as_slice);
            for &op in victims {
                let id = op.id();
                if id == removal.op.id() || before.contains(&id) || after.contains(&id) {
                    continue;
                }
                if owner == Some(self.author_of(op)) {
                    continue;
                }
                let Some(group) = self.voidable_group(op) else {
                    continue;
                };
                let reaches = removal
                    .group
                    .is_none_or(|lost| self.group_and_ancestors(group).contains(&lost));
                if !reaches {
                    continue;
                }
                // Two admins removing each other both stay removed.
                let mutual = matches!(removal.target, Target::Account(target) if
                    removes_account(op) == Some(removal.op.author()) && target == self.author_of(op));
                if mutual {
                    continue;
                }
                let _ = next.insert(id);
            }
        }

        next.extend(self.cascade(void));
        next
    }

    /// The removals that are not void: member removals and revocations, and the
    /// role changes that took an admin's role away.
    fn effective_removals(&self, void: &BTreeSet<[u8; 32]>) -> Vec<Removal<'a>> {
        let mut out: Vec<Removal<'a>> = self
            .removals
            .iter()
            .filter(|removal| !void.contains(&removal.op.id()))
            .copied()
            .collect();
        for &(op, group, member) in &self.demotion_candidates {
            if void.contains(&op.id()) {
                continue;
            }
            // Past the budget a role change counts as a demotion: the one it
            // might be is the one that matters.
            let was_admin = self.over_budget() || {
                let before = self.ancestors_of(op.id());
                let view = self.fold(
                    before
                        .iter()
                        .filter(|id| !void.contains(*id))
                        .filter_map(|id| self.by_id.get(id).copied()),
                );
                view.is_group_admin(&member, group)
            };
            if was_admin {
                out.push(Removal {
                    op,
                    target: Target::Account(member),
                    group: Some(group),
                });
            }
        }
        out
    }

    /// Ops whose signer held its authority through a grant a void op made, and
    /// holds none once that grant is taken out.
    fn cascade(&self, void: &BTreeSet<[u8; 32]>) -> BTreeSet<[u8; 32]> {
        let mut out = BTreeSet::new();
        let mut granted: BTreeSet<AccountId> = BTreeSet::new();
        for id in void {
            match self.by_id.get(id).map(|op| &op.payload) {
                Some(OpPayload::MemberAdded { member, .. })
                | Some(OpPayload::MemberCapabilitySet { member, .. }) => {
                    let _ = granted.insert(*member);
                }
                Some(OpPayload::SubgroupCreated { admin, .. }) => {
                    let _ = granted.insert(*admin);
                }
                Some(OpPayload::AdminChanged { new_admin }) => {
                    let _ = granted.insert(*new_admin);
                }
                _ => {}
            }
        }
        if granted.is_empty() {
            return out;
        }

        // Ops with a void op behind them, found in one pass in causal order.
        let mut ordered: Vec<&Op> = self.by_id.values().copied().collect();
        ordered
            .sort_unstable_by_key(|op| (self.depth.get(&op.id()).copied().unwrap_or(0), op.id()));
        let mut behind_void: HashSet<[u8; 32]> = HashSet::new();
        let mut candidates: Vec<&Op> = Vec::new();
        let owner = self.base.root.map(|(_, admin)| admin);
        for op in ordered {
            let tainted = op
                .parents
                .iter()
                .any(|parent| void.contains(parent) || behind_void.contains(parent));
            if !tainted {
                continue;
            }
            let _ = behind_void.insert(op.id());
            // An op already void is judged again, so a round never forgets what the
            // round before established.
            if granted.contains(&self.author_of(op))
                && owner != Some(self.author_of(op))
                && self.voidable_group(op).is_some()
            {
                candidates.push(op);
            }
        }

        for op in candidates {
            if self.over_budget() {
                let _ = out.insert(op.id());
                continue;
            }
            let Some(group) = self.voidable_group(op) else {
                continue;
            };
            let before = self.ancestors_of(op.id());
            let held_view = self.fold(before.iter().filter_map(|id| self.by_id.get(id).copied()));
            let was_bound = self.device_bound(&held_view, op);
            if !self.holds(&held_view, op, group, was_bound) {
                continue;
            }
            let without = self.fold(
                before
                    .iter()
                    .filter(|id| !void.contains(*id))
                    .filter_map(|id| self.by_id.get(id).copied()),
            );
            if !self.holds(&without, op, group, was_bound) {
                let _ = out.insert(op.id());
            }
        }
        out
    }

    fn device_bound(&self, view: &AclView, op: &Op) -> bool {
        view.devices.values().any(|binding| {
            binding.account == self.author_of(op) && binding.sign_pk == *op.device_key()
        })
    }

    /// Does `op`'s signer hold, in `view`, the authority the op acts on?
    fn holds(&self, view: &AclView, op: &Op, group: ContextGroupId, was_bound: bool) -> bool {
        let account = self.author_of(op);
        if view.revoked_devices.contains(&self.device_of(op)) {
            return false;
        }
        if was_bound && !self.device_bound(view, op) {
            return false;
        }
        if view.is_authorized_admin(group, &account, self.base.root) {
            return true;
        }
        let member = view
            .groups
            .get(&group)
            .is_some_and(|members| members.contains_key(&account));
        let folded = view.capability(&group, &account);
        let effective = if folded != 0 {
            folded
        } else {
            self.base.default_cap_base
        };
        member && effective != 0
    }
}
