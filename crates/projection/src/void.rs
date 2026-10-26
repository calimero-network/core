//! Ops that carry no authority because their signer was removed, demoted or
//! stripped of the capability they relied on, concurrently.
//! The void set depends only on the set of ops, never on their arrival order.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::rc::Rc;

use calimero_account::{AccountId, DeviceId};
use calimero_authz::AclView;
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_op::{Authorship, Op, OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;

use calimero_storage::shared_writers::OverBudget;

use crate::ScopeState;

/// Rounds of the search before its tail is treated as a cycle.
const MAX_ROUNDS: usize = 32;

/// Ops folded judging cascades; past it the remaining candidates are void.
/// Their signers' standing rests on a void op, and every node must agree.
/// The fold work a void judgement may spend before it is undecidable.
pub const MAX_FOLD_WORK: usize = 50_000_000;

const FOLD_WORK_PER_OP: usize = 10_000; // keeps a big honest log decidable

/// Facts the authority check reads that no op carries, written by the store at genesis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AuthorityBase {
    /// The namespace root group and its founder, whom a transfer at a cut replaces.
    pub root: Option<(ContextGroupId, AccountId)>,
    /// The root group's default member capabilities.
    pub default_cap_base: u32,
}

/// Where an op acts, and the capability bits that admit a non-admin to it, for an
/// op whose payload says neither (one the projection models nothing about).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Acting {
    /// The group the op acts in.
    pub group: ContextGroupId,
    /// The bits a member who is no admin needs for the op; `0` when none admits one.
    pub capability: u32,
}

impl From<ContextGroupId> for Acting {
    /// An op in `group` that no capability admits.
    fn from(group: ContextGroupId) -> Self {
        Self {
            group,
            capability: 0,
        }
    }
}

type IdSet = Rc<HashSet<[u8; 32]>>;

/// The ids of the two ops a group tree is folded for.
type TreeKey = ([u8; 32], [u8; 32]);

/// Each subgroup's parent and the Restricted subgroups, the latest creation, reparent or
/// visibility change winning as the fold does. The namespace root is always open.
struct GroupTree {
    parent: BTreeMap<ContextGroupId, ContextGroupId>,
    restricted: BTreeSet<ContextGroupId>,
}

impl GroupTree {
    fn fold<'o>(
        ops: impl Iterator<Item = &'o Op>,
        depth: &HashMap<[u8; 32], u32>,
        root: ContextGroupId,
    ) -> Self {
        type Stamped<T> = ((u32, [u8; 32]), T);
        let group = |scope: &ScopeId| ContextGroupId::from(*scope.as_bytes());
        let mut latest: BTreeMap<ContextGroupId, Stamped<ContextGroupId>> = BTreeMap::new();
        let mut visibility: BTreeMap<ContextGroupId, Stamped<bool>> = BTreeMap::new();
        for op in ops {
            let stamp = (depth.get(&op.id()).copied().unwrap_or(0), op.id());
            let (parented, shown) = match &op.payload {
                OpPayload::SubgroupCreated {
                    child,
                    parent,
                    restricted,
                    ..
                } => (Some((child, parent)), Some((child, *restricted))),
                OpPayload::SubgroupReparented { child, new_parent } => {
                    (Some((child, new_parent)), None)
                }
                OpPayload::SubgroupVisibilitySet { scope, restricted } => {
                    (None, Some((scope, *restricted)))
                }
                _ => continue,
            };
            if let Some((child, parent)) = parented {
                let child = group(child);
                if latest.get(&child).is_none_or(|(held, _)| stamp > *held) {
                    let _ = latest.insert(child, (stamp, group(parent)));
                }
            }
            if let Some((scope, restricted)) = shown {
                let scope = group(scope);
                if visibility.get(&scope).is_none_or(|(held, _)| stamp > *held) {
                    let _ = visibility.insert(scope, (stamp, restricted));
                }
            }
        }
        Self {
            parent: latest
                .into_iter()
                .map(|(child, (_, parent))| (child, parent))
                .collect(),
            restricted: visibility
                .into_iter()
                .filter_map(|(scope, (_, restricted))| {
                    (restricted && scope != root).then_some(scope)
                })
                .collect(),
        }
    }

    /// `group` and the groups above it.
    fn chain(&self, group: ContextGroupId) -> Vec<ContextGroupId> {
        let mut chain = vec![group];
        let mut at = group;
        while let Some(parent) = self.parent.get(&at) {
            if chain.contains(parent) || chain.len() > 64 {
                break;
            }
            chain.push(*parent);
            at = *parent;
        }
        chain
    }

    /// The chain from `group` up to and including its private space; the whole chain in
    /// the open space.
    fn within_space(&self, group: ContextGroupId) -> Vec<ContextGroupId> {
        let mut chain = self.chain(group);
        if let Some(space) = chain.iter().position(|g| self.restricted.contains(g)) {
            chain.truncate(space + 1);
        }
        chain
    }

    /// The nearest Restricted ancestor-or-self of `group`, if any.
    fn private_space(&self, group: ContextGroupId) -> Option<ContextGroupId> {
        self.chain(group)
            .into_iter()
            .find(|g| self.restricted.contains(g))
    }
}

#[derive(Clone, Copy)]
enum Target {
    Account(AccountId),
    Device(DeviceId),
}

/// A capability set that may take bits away from another member.
#[derive(Clone, Copy)]
struct CapabilityRevoke<'a> {
    op: &'a Op,
    group: ContextGroupId,
    member: AccountId,
    /// The bits the member holds after it.
    kept: u32,
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
    /// The ops of `log` that carry no authority; `Err` past the fold budget, which no
    /// node may answer by guessing.
    pub fn void_ops(log: &[Op], base: AuthorityBase) -> Result<BTreeSet<[u8; 32]>, OverBudget> {
        Self::void_ops_with(log, base, None)
    }

    /// [`Self::void_ops`] over `log` plus `candidate`, an op about to join it.
    /// [`Acting`] names where it acts, and what admits it, when its payload does not;
    /// its cut must be whole.
    pub fn void_ops_with(
        log: &[Op],
        base: AuthorityBase,
        candidate: Option<(&Op, Option<Acting>)>,
    ) -> Result<BTreeSet<[u8; 32]>, OverBudget> {
        Self::void_ops_judging(log, base, candidate, &[])
    }

    /// [`Self::void_ops_with`] that also judges `held`, ops of `log` the projection
    /// models nothing about, each with the group it acted in.
    pub fn void_ops_judging(
        log: &[Op],
        base: AuthorityBase,
        candidate: Option<(&Op, Option<Acting>)>,
        held: &[([u8; 32], ContextGroupId)],
    ) -> Result<BTreeSet<[u8; 32]>, OverBudget> {
        Self::void_ops_bounded(log, base, candidate, held, fold_budget_for(log.len()))
    }

    /// [`Self::void_ops_judging`] with an explicit bound on the ops folded.
    pub fn void_ops_bounded(
        log: &[Op],
        base: AuthorityBase,
        candidate: Option<(&Op, Option<Acting>)>,
        held: &[([u8; 32], ContextGroupId)],
        budget: usize,
    ) -> Result<BTreeSet<[u8; 32]>, OverBudget> {
        let mut ops: Vec<&Op> = log.iter().collect();
        let mut explicit: HashMap<[u8; 32], Acting> = held
            .iter()
            .map(|(id, group)| (*id, Acting::from(*group)))
            .collect();
        if let Some((op, acting)) = candidate {
            if let Some(acting) = acting {
                let _ = explicit.insert(op.id(), acting);
            }
            if !log.iter().any(|held| held.id() == op.id()) {
                ops.push(op);
            }
        }
        // Most logs hold no removal that could matter; say so before building anything.
        let resolved = resolve_unattributed(&ops);
        if !holds_removal(&ops, &resolved, base) {
            return Ok(BTreeSet::new());
        }
        Analysis::new(&ops, base, explicit, resolved, budget).run()
    }
}

/// The fold work a void judgement over `ops` ops may spend: the floor, or more as the log grows.
pub fn fold_budget_for(ops: usize) -> usize {
    MAX_FOLD_WORK.max(ops.saturating_mul(FOLD_WORK_PER_OP))
}

/// The group `op` acts in, for payloads whose authority a removal can take away.
pub(crate) fn payload_group(op: &Op) -> Option<ContextGroupId> {
    let root = || ContextGroupId::from(*op.scope.as_bytes());
    let mut payload = &op.payload;
    while let OpPayload::RootGuarded { carried, .. } = payload {
        payload = carried;
    }
    match payload {
        OpPayload::MemberAdded { group, .. }
        | OpPayload::MemberRemoved { group, .. }
        | OpPayload::MemberLeft { group, .. }
        | OpPayload::MemberCapabilitySet { group, .. }
        | OpPayload::DefaultCapabilitiesSet { group, .. }
        | OpPayload::TeeAuthoringPolicySet { group, .. }
        | OpPayload::TeeAuthorityEvidence { group, .. }
        | OpPayload::SharedWritersRotated { group, .. }
        | OpPayload::TargetApplicationSet { group, .. }
        | OpPayload::CascadeUpgraded { group, .. } => Some(*group),
        // Opening rests on namespace standing, so only a removal every member can read
        // voids it: one from the subgroup is sealed from members outside it.
        OpPayload::SubgroupVisibilitySet {
            restricted: false, ..
        } => Some(root()),
        OpPayload::SubgroupVisibilitySet { scope, .. } => {
            Some(ContextGroupId::from(*scope.as_bytes()))
        }
        OpPayload::SubgroupCreated { parent, .. } => Some(ContextGroupId::from(*parent.as_bytes())),
        OpPayload::SubgroupReparented { .. }
        | OpPayload::SubgroupDeleted { .. }
        | OpPayload::AdminChanged { .. }
        | OpPayload::DeviceRevoked { .. }
        | OpPayload::PolicyUpdated { .. } => Some(root()),
        _ => None,
    }
}

/// The account `op` would be removing, for the mutual-removal exemption.
fn removes_account(op: &Op) -> Option<AccountId> {
    match &op.payload {
        OpPayload::MemberRemoved { member, .. } | OpPayload::MemberLeft { member, .. } => {
            Some(*member)
        }
        OpPayload::MemberAdded { member, role, .. } if !matches!(role, GroupMemberRole::Admin) => {
            Some(*member)
        }
        OpPayload::DeviceRevoked { account, .. } => Some(*account),
        _ => None,
    }
}

/// Does `ops` hold an op that takes authority away from someone else: a member
/// removal, a device revocation, a role change for an account granted `Admin`, or a
/// capability set leaving out a bit the member could have held?
pub(crate) fn holds_removal(ops: &[&Op], resolved: &Resolved, base: AuthorityBase) -> bool {
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
    let mut admin_grants: HashSet<(ContextGroupId, AccountId)> = HashSet::new();
    let mut role_changes: Vec<(ContextGroupId, AccountId)> = Vec::new();
    // Every bit each member was ever granted, and every default each group set.
    let mut granted: HashMap<(ContextGroupId, AccountId), u32> = HashMap::new();
    let mut defaults: HashMap<ContextGroupId, u32> = HashMap::new();
    let mut capability_sets: Vec<(ContextGroupId, AccountId, u32)> = Vec::new();
    for op in ops {
        if let Some(founder) = crate::seated_founder(op, &op.payload) {
            let _ = admin_grants.insert((ContextGroupId::from(*op.scope.as_bytes()), founder));
        }
        match &op.payload {
            OpPayload::MemberCapabilitySet {
                group,
                member,
                capabilities,
            } => {
                *granted.entry((*group, *member)).or_default() |= capabilities.bits();
            }
            OpPayload::DefaultCapabilitiesSet {
                group,
                capabilities,
            } => {
                *defaults.entry(*group).or_default() |= capabilities.bits();
            }
            _ => {}
        }
        if author(op) == Authorship::UNATTRIBUTED_ACCOUNT {
            continue;
        }
        match &op.payload {
            OpPayload::MemberCapabilitySet {
                group,
                member,
                capabilities,
            } if author(op) != *member => {
                capability_sets.push((*group, *member, capabilities.bits()));
            }
            OpPayload::MemberRemoved { member, .. } | OpPayload::MemberLeft { member, .. }
                if author(op) != *member =>
            {
                return true
            }
            OpPayload::DeviceRevoked { device: target, .. } if device(op) != *target => {
                return true
            }
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
            OpPayload::MemberAdded { group, member, .. } if author(op) != *member => {
                role_changes.push((*group, *member));
            }
            _ => {}
        }
    }
    role_changes
        .iter()
        .any(|change| admin_grants.contains(change))
        || capability_sets.iter().any(|(group, member, kept)| {
            let held = granted.get(&(*group, *member)).copied().unwrap_or(0)
                | defaults.get(group).copied().unwrap_or(0)
                | base.default_cap_base;
            held & !kept != 0
        })
}

/// The capability bits that admit a member who is no admin to `op`, as its payload
/// names them; `0` when none does. Mirrors the apply's `require_*` gates.
fn capability_by_payload(op: &Op, author: AccountId) -> u32 {
    match &op.payload {
        // Making an admin takes an admin; leaving takes nothing.
        OpPayload::MemberAdded { role, .. } if !matches!(role, GroupMemberRole::Admin) => {
            MemberCapabilities::MANAGE_MEMBERS.bits()
        }
        OpPayload::MemberRemoved { member, .. } | OpPayload::MemberLeft { member, .. }
            if *member != author =>
        {
            MemberCapabilities::MANAGE_MEMBERS.bits()
        }
        OpPayload::SubgroupCreated { .. } => MemberCapabilities::CAN_CREATE_SUBGROUP.bits(),
        OpPayload::SubgroupDeleted { .. } => MemberCapabilities::CAN_DELETE_SUBGROUP.bits(),
        OpPayload::TargetApplicationSet { .. } | OpPayload::CascadeUpgraded { .. } => {
            MemberCapabilities::MANAGE_APPLICATION.bits()
        }
        // Opening is the creator's alone and takes no capability (#4522).
        OpPayload::SubgroupVisibilitySet {
            restricted: true, ..
        } => MemberCapabilities::CAN_MANAGE_VISIBILITY.bits(),
        _ => 0,
    }
}

/// The account `op` hands the namespace to, as the fold reads an `AdminChanged`.
fn transfer_to(op: &Op) -> Option<AccountId> {
    match &op.payload {
        OpPayload::AdminChanged { new_admin } => Some(*new_admin),
        OpPayload::RootGuarded {
            carried, account, ..
        } if *account == op.authorship.account => match carried.as_ref() {
            OpPayload::AdminChanged { new_admin } => Some(*new_admin),
            _ => None,
        },
        _ => None,
    }
}

/// The account and device of each op built without an author, by op id.
pub(crate) type Resolved = HashMap<[u8; 32], (AccountId, DeviceId)>;

/// The account and device of each op built without an author, from the log's device
/// links (a revocation drops the binding, not the link). A key linked twice names none.
fn resolve_unattributed(ops: &[&Op]) -> Resolved {
    if !ops
        .iter()
        .any(|op| op.author() == Authorship::UNATTRIBUTED_ACCOUNT)
    {
        return HashMap::new();
    }
    let mut linked: HashMap<[u8; 32], Option<(AccountId, DeviceId)>> = HashMap::new();
    for op in ops {
        let payload = match &op.payload {
            OpPayload::AccountNamespaceFounded { carried, .. }
            | OpPayload::NamespaceFounded { carried, .. } => carried,
            payload => payload,
        };
        let (OpPayload::DeviceLinked { cert, .. } | OpPayload::MemberJoinedWithDevice { cert, .. }) =
            payload
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
    resolved: Resolved,
    /// Whether each device revocation was entitled, while no void op is behind it.
    entitled: RefCell<HashMap<[u8; 32], bool>>,
    by_id: HashMap<[u8; 32], &'a Op>,
    /// Where the candidate and the held ops act, and what admits them, when their
    /// payloads say neither.
    explicit: HashMap<[u8; 32], Acting>,
    /// Removals that need no judgement: a member removal or a device revocation.
    removals: Vec<Removal<'a>>,
    /// Role changes to a non-admin role for an account that was granted `Admin`;
    /// a demotion only when the account was an admin at the change's cut.
    demotion_candidates: Vec<(&'a Op, ContextGroupId, AccountId)>,
    /// Transfers of the namespace, with the account each hands it to.
    transfers: Vec<(&'a Op, AccountId)>,
    /// Capability sets for another member; a revocation of the bits the member held
    /// at the set's cut and no longer holds, when an admin made it.
    capability_revokes: Vec<CapabilityRevoke<'a>>,
    /// The bits each capability set took away, while no void op is behind it.
    revoked: RefCell<HashMap<[u8; 32], u32>>,
    /// Whether each op's author was an admin where it acts, at its cut, while no
    /// void op is behind it.
    admin_at_cut: RefCell<HashMap<[u8; 32], bool>>,
    /// Voidable ops by author and by device.
    by_account: HashMap<AccountId, Vec<&'a Op>>,
    by_device: HashMap<DeviceId, Vec<&'a Op>>,
    depth: HashMap<[u8; 32], u32>,
    children: HashMap<[u8; 32], Vec<[u8; 32]>>,
    /// The ops that place a group or set its visibility.
    tree_ops: Vec<&'a Op>,
    /// The tree each judged op (and removal) pair sees, memoised for the round in progress.
    trees: RefCell<HashMap<TreeKey, Rc<GroupTree>>>,
    ancestors: RefCell<HashMap<[u8; 32], IdSet>>,
    descendants: RefCell<HashMap<[u8; 32], IdSet>>,
    work: Cell<usize>,
}

impl<'a> Analysis<'a> {
    fn new(
        ops: &[&'a Op],
        base: AuthorityBase,
        explicit: HashMap<[u8; 32], Acting>,
        resolved: Resolved,
        budget: usize,
    ) -> Self {
        let by_id: HashMap<[u8; 32], &Op> = ops.iter().map(|op| (op.id(), *op)).collect();

        let mut admin_grants: HashSet<(ContextGroupId, AccountId)> = HashSet::new();
        for op in ops {
            if let Some(founder) = crate::seated_founder(op, &op.payload) {
                let _ = admin_grants.insert((ContextGroupId::from(*op.scope.as_bytes()), founder));
            }
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
        let mut capability_revokes = Vec::new();
        let transfers = ops
            .iter()
            .filter_map(|&op| transfer_to(op).map(|heir| (op, heir)))
            .collect();
        for &op in ops {
            if !attributed(op) {
                continue;
            }
            match &op.payload {
                OpPayload::MemberCapabilitySet {
                    group,
                    member,
                    capabilities,
                } if author(op) != *member => {
                    capability_revokes.push(CapabilityRevoke {
                        op,
                        group: *group,
                        member: *member,
                        kept: capabilities.bits(),
                    });
                }
                OpPayload::MemberRemoved { group, member }
                | OpPayload::MemberLeft { group, member }
                    if author(op) != *member =>
                {
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
            entitled: RefCell::default(),
            by_id,
            explicit,
            removals,
            demotion_candidates,
            transfers,
            capability_revokes,
            revoked: RefCell::default(),
            admin_at_cut: RefCell::default(),
            by_account: HashMap::new(),
            by_device: HashMap::new(),
            depth: HashMap::new(),
            children: HashMap::new(),
            tree_ops: Vec::new(),
            trees: RefCell::default(),
            ancestors: RefCell::default(),
            descendants: RefCell::default(),
            work: Cell::new(0),
        };
        if analysis.removals.is_empty()
            && analysis.demotion_candidates.is_empty()
            && analysis.capability_revokes.is_empty()
        {
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
        analysis.tree_ops = ops
            .iter()
            .copied()
            .filter(|op| {
                matches!(
                    op.payload,
                    OpPayload::SubgroupCreated { .. }
                        | OpPayload::SubgroupReparented { .. }
                        | OpPayload::SubgroupVisibilitySet { .. }
                )
            })
            .collect();

        // Causal order, so what a spent budget leaves unjudged does not follow the log's.
        let mut removals = std::mem::take(&mut analysis.removals);
        removals.sort_unstable_by_key(|removal| {
            let id = removal.op.id();
            (analysis.depth.get(&id).copied().unwrap_or(0), id)
        });
        analysis.removals = removals;
        let mut demotions = std::mem::take(&mut analysis.demotion_candidates);
        demotions.sort_unstable_by_key(|(op, _, _)| {
            let id = op.id();
            (analysis.depth.get(&id).copied().unwrap_or(0), id)
        });
        analysis.demotion_candidates = demotions;
        let mut revokes = std::mem::take(&mut analysis.capability_revokes);
        revokes.sort_unstable_by_key(|revoke| {
            let id = revoke.op.id();
            (analysis.depth.get(&id).copied().unwrap_or(0), id)
        });
        analysis.capability_revokes = revokes;
        // Causal order too, so every node judges the victims alike.
        let depth = &analysis.depth;
        for victims in analysis
            .by_account
            .values_mut()
            .chain(analysis.by_device.values_mut())
        {
            victims.sort_unstable_by_key(|op| {
                let id = op.id();
                (depth.get(&id).copied().unwrap_or(0), id)
            });
        }
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
        payload_group(op).or_else(|| self.explicit.get(&op.id()).map(|acting| acting.group))
    }

    /// The capability bits that admit a member who is no admin to `op`.
    fn capability_needed(&self, op: &Op) -> u32 {
        capability_by_payload(op, self.author_of(op))
            | self
                .explicit
                .get(&op.id())
                .map_or(0, |acting| acting.capability)
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

    /// The group tree `op` saw at its cut: the placements and visibility changes in its
    /// ancestry that are not void. Concurrent or void ones never count, so no signer
    /// can move itself out of a removal's reach by reshaping the tree beside it.
    fn tree_at(&self, op: &Op, void: &BTreeSet<[u8; 32]>) -> Rc<GroupTree> {
        let key = (op.id(), op.id());
        if let Some(known) = self.trees.borrow().get(&key) {
            return Rc::clone(known);
        }
        self.work.set(self.work.get() + self.tree_ops.len().max(1));
        let before = self.ancestors_of(op.id());
        let seen = self
            .tree_ops
            .iter()
            .copied()
            .filter(|held| before.contains(&held.id()) && !void.contains(&held.id()));
        let tree = Rc::new(GroupTree::fold(seen, &self.depth, self.root_of(op)));
        let _ = self.trees.borrow_mut().insert(key, Rc::clone(&tree));
        tree
    }

    /// The tree over what `op` and `removal` each saw, so neither can hide a placement
    /// from the other. The removed signer's placements concurrent with the removal are
    /// left out: the removal voids them, and they must not move the op out of its reach.
    fn tree_across(
        &self,
        op: &Op,
        removal: &Removal<'_>,
        void: &BTreeSet<[u8; 32]>,
    ) -> Rc<GroupTree> {
        let key = (op.id(), removal.op.id());
        if let Some(known) = self.trees.borrow().get(&key) {
            return Rc::clone(known);
        }
        self.work.set(self.work.get() + self.tree_ops.len().max(1));
        let (before, also) = (
            self.ancestors_of(op.id()),
            self.ancestors_of(removal.op.id()),
        );
        let after = self.descendants_of(removal.op.id());
        let removed = |held: &Op| match removal.target {
            Target::Account(account) => self.author_of(held) == account,
            Target::Device(device) => self.device_of(held) == device,
        };
        let seen = self.tree_ops.iter().copied().filter(|held| {
            let id = held.id();
            (before.contains(&id) || also.contains(&id))
                && !void.contains(&id)
                && !(removed(held) && !also.contains(&id) && !after.contains(&id))
        });
        let tree = Rc::new(GroupTree::fold(seen, &self.depth, self.root_of(op)));
        let _ = self.trees.borrow_mut().insert(key, Rc::clone(&tree));
        tree
    }

    /// The group whose members `op` acts among when a removal is weighed against it:
    /// its own, or for a make-private the group it is carved out of.
    fn reach_start(
        &self,
        op: &Op,
        group: ContextGroupId,
        tree: &GroupTree,
    ) -> Option<ContextGroupId> {
        match op.payload {
            OpPayload::SubgroupVisibilitySet {
                restricted: true, ..
            } => tree.parent.get(&group).copied(),
            _ => Some(group),
        }
    }

    /// The namespace root: the genesis fact when the store gave one, else the scope.
    fn root_of(&self, op: &Op) -> ContextGroupId {
        self.base.root.map_or_else(
            || ContextGroupId::from(*op.scope.as_bytes()),
            |(group, _)| group,
        )
    }

    fn is_owner(&self, op: &Op) -> bool {
        self.owner_at(op) == Some(self.author_of(op))
    }

    /// Whether `op` is the owner's in a group outside any private space: no removal or
    /// revocation but its own takes that authority.
    fn owner_exempt(&self, op: &Op, group: ContextGroupId, void: &BTreeSet<[u8; 32]>) -> bool {
        self.is_owner(op) && self.tree_at(op, void).private_space(group).is_none()
    }

    /// The namespace's owner at `op`'s cut: the heir of the latest transfer behind it, as the
    /// fold picks it, else the founder. Never void: an owner's ops are exempt.
    fn owner_at(&self, op: &Op) -> Option<AccountId> {
        let before = self.ancestors_of(op.id());
        self.transfers
            .iter()
            .filter(|(transfer, _)| before.contains(&transfer.id()))
            .max_by_key(|(transfer, _)| {
                let id = transfer.id();
                (transfer.hlc, self.depth.get(&id).copied().unwrap_or(0), id)
            })
            .map(|(_, heir)| *heir)
            .or_else(|| self.base.root.map(|(_, founder)| founder))
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

    fn run(&self) -> Result<BTreeSet<[u8; 32]>, OverBudget> {
        let mut history: Vec<BTreeSet<[u8; 32]>> = vec![BTreeSet::new()];
        loop {
            let current = &history[history.len() - 1];
            let next = self.round(current);
            if self.over_budget() {
                return Err(OverBudget);
            }
            if next == *current {
                return Ok(next);
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
                return Ok(union);
            }
            history.push(next);
        }
    }

    /// One application of the rule to the void set `void`.
    fn round(&self, void: &BTreeSet<[u8; 32]>) -> BTreeSet<[u8; 32]> {
        self.trees.borrow_mut().clear();
        let mut next = BTreeSet::new();

        for removal in self.effective_removals(void) {
            let before = self.ancestors_of(removal.op.id());
            let after = self.descendants_of(removal.op.id());
            let victims: &[&Op] = match removal.target {
                Target::Account(account) => self.by_account.get(&account),
                Target::Device(device) => self.by_device.get(&device),
            }
            .map_or(&[], Vec::as_slice);
            for &op in victims {
                if self.over_budget() {
                    return next;
                }
                let id = op.id();
                if id == removal.op.id() || before.contains(&id) || after.contains(&id) {
                    continue;
                }
                let Some(group) = self.voidable_group(op) else {
                    continue;
                };
                // Two admins removing each other both stay removed.
                let mutual = removes_account(op) == Some(self.author_of(removal.op))
                    && match removal.target {
                        Target::Account(account) => account == self.author_of(op),
                        Target::Device(device) => device == self.device_of(op),
                    };
                if mutual {
                    continue;
                }
                let tree = self.tree_at(op, void);
                let start = self.reach_start(op, group, &tree);
                let space = start.and_then(|start| tree.private_space(start));
                let owner = self.is_owner(op);
                if owner && space.is_none() {
                    continue;
                }
                // The owner is judged in a private space by removals from that space and
                // by its own device revocations, not by anyone's root-level action.
                let reaches = match (owner, removal.group) {
                    (true, Some(lost)) => Some(lost) == space,
                    (true, None) => self.author_of(removal.op) == self.author_of(op),
                    (false, None) => true,
                    (false, Some(lost)) => {
                        let both = self.tree_across(op, &removal, void);
                        lost == self.root_of(op)
                            || self
                                .reach_start(op, group, &both)
                                .is_some_and(|start| both.within_space(start).contains(&lost))
                    }
                };
                if !reaches {
                    continue;
                }
                let _ = next.insert(id);
            }
        }

        for revoke in &self.capability_revokes {
            if void.contains(&revoke.op.id()) {
                continue;
            }
            let removed = self.revoked_bits(revoke, void);
            if removed == 0 {
                continue;
            }
            let Some(victims) = self.by_account.get(&revoke.member) else {
                continue;
            };
            let before = self.ancestors_of(revoke.op.id());
            let after = self.descendants_of(revoke.op.id());
            for &op in victims {
                let id = op.id();
                if before.contains(&id) || after.contains(&id) {
                    continue;
                }
                // A capability is held in its own group: it is never inherited.
                if self.voidable_group(op) != Some(revoke.group)
                    || self.capability_needed(op) & removed == 0
                {
                    continue;
                }
                if self.owner_exempt(op, revoke.group, void) {
                    continue;
                }
                // An admin at the op's cut needed no capability for it.
                if !self.was_admin(op, revoke.group, void) {
                    let _ = next.insert(id);
                }
            }
        }

        next.extend(self.cascade(void));
        next
    }

    /// The bits `revoke` took from its member: those the member held at its cut and
    /// does not keep. `0` unless an admin made it. Past the budget, every bit it
    /// does not keep: the revocation it might be is the one that matters.
    fn revoked_bits(&self, revoke: &CapabilityRevoke<'_>, void: &BTreeSet<[u8; 32]>) -> u32 {
        let id = revoke.op.id();
        let before = self.ancestors_of(id);
        let cacheable = !void.iter().any(|voided| before.contains(voided));
        if cacheable {
            if let Some(known) = self.revoked.borrow().get(&id) {
                return *known;
            }
        }
        let view = self.fold(
            before
                .iter()
                .filter(|held| !void.contains(*held))
                .filter_map(|held| self.by_id.get(held).copied()),
        );
        let author = self.author_of(revoke.op);
        let removed = if view.is_authorized_admin(revoke.group, &author, self.base.root) {
            view.capability(&revoke.group, &revoke.member, self.base.default_cap_base)
                & !revoke.kept
        } else {
            0
        };
        if cacheable {
            let _ = self.revoked.borrow_mut().insert(id, removed);
        }
        removed
    }

    /// Was the author of `op` an admin of `group` at its cut? Past the budget, no:
    /// the op is then judged as one a capability admitted.
    fn was_admin(&self, op: &Op, group: ContextGroupId, void: &BTreeSet<[u8; 32]>) -> bool {
        let id = op.id();
        let before = self.ancestors_of(id);
        let cacheable = !void.iter().any(|voided| before.contains(voided));
        if cacheable {
            if let Some(known) = self.admin_at_cut.borrow().get(&id) {
                return *known;
            }
        }
        let view = self.fold(
            before
                .iter()
                .filter(|held| !void.contains(*held))
                .filter_map(|held| self.by_id.get(held).copied()),
        );
        let admin = view.is_authorized_admin(group, &self.author_of(op), self.base.root);
        if cacheable {
            let _ = self.admin_at_cut.borrow_mut().insert(id, admin);
        }
        admin
    }

    /// The removals that are not void: member removals and revocations, and the
    /// role changes that took an admin's role away.
    fn effective_removals(&self, void: &BTreeSet<[u8; 32]>) -> Vec<Removal<'a>> {
        let mut out: Vec<Removal<'a>> = self
            .removals
            .iter()
            .filter(|removal| !void.contains(&removal.op.id()))
            .filter(|removal| self.entitled(removal, void))
            .copied()
            .collect();
        for &(op, group, member) in &self.demotion_candidates {
            if void.contains(&op.id()) {
                continue;
            }
            let was_admin = {
                let before = self.ancestors_of(op.id());
                let view = self.fold(
                    before
                        .iter()
                        .filter(|id| !void.contains(*id))
                        .filter_map(|id| self.by_id.get(id).copied()),
                );
                // The apply logs a role change it took no action on (a TEE admitted
                // over a member), so the author must have been an admin too.
                view.is_group_admin(&member, group)
                    && view.is_authorized_admin(group, &self.author_of(op), self.base.root)
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

    /// Was the author of `removal` entitled to it at its own cut? Asked of a device
    /// revocation, which the apply logs even refused. Only a root-group admin counts.
    fn entitled(&self, removal: &Removal<'_>, void: &BTreeSet<[u8; 32]>) -> bool {
        let (Target::Device(_), OpPayload::DeviceRevoked { .. }) =
            (removal.target, &removal.op.payload)
        else {
            return true;
        };
        let id = removal.op.id();
        let before = self.ancestors_of(id);
        // With no void op behind it the answer is the same in every round.
        let cacheable = !void.iter().any(|voided| before.contains(voided));
        if cacheable {
            if let Some(known) = self.entitled.borrow().get(&id) {
                return *known;
            }
        }
        let view = self.fold(
            before
                .iter()
                .filter(|held| !void.contains(*held))
                .filter_map(|held| self.by_id.get(held).copied()),
        );
        let entitled = self.is_admin_at(&view, removal.op);
        if cacheable {
            let _ = self.entitled.borrow_mut().insert(id, entitled);
        }
        entitled
    }

    /// Was the author of `op` an admin of the root group in `view`?
    fn is_admin_at(&self, view: &AclView, op: &Op) -> bool {
        let author = self.author_of(op);
        view.root_at_cut(self.base.root)
            .is_some_and(|(_, owner)| owner == author)
            || view.is_root_admin(&author)
            || view.is_group_admin(&author, ContextGroupId::from(*op.scope.as_bytes()))
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
                && self
                    .voidable_group(op)
                    .is_some_and(|group| !self.owner_exempt(op, group, void))
            {
                candidates.push(op);
            }
        }

        for op in candidates {
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
        let effective = view.capability(&group, &account, self.base.default_cap_base);
        member && effective != 0
    }
}
