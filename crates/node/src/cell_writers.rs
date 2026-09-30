//! Who may write the `SharedStorage` cells a peer's delta or a repair touches.
//!
//! A cell's writers are the governance fold's answer at a cut, never what the delta or this
//! node's stored copy of the cell says. A peer's delta is judged at the governance position its
//! author signed, so a write made before a rotation is accepted after it and a write that
//! claims a position older than its own causal past is refused.
//!
//! A position is **stale** when it does not cover (is not the same cut as, or a descendant of)
//! the position of a data-DAG parent this node stores: the parent's author had already seen
//! governance up to that position, so a child that cites less is reaching back to a time
//! before a rotation it has causally seen. It matters only for a delta that touches a cell
//! that has rotated, at the delta's cut or at this node's current heads.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, RwLock};

use calimero_context::scope_projection::ScopeProjections;
use calimero_context_client::client::CurrentCellWriters;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextId;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::entities::StorageType;
use calimero_storage::shared_writers::{CellWriters, Writers, WritersUnavailable};
use calimero_store::Store;
use tracing::debug;

/// The decision on a delta's shared cells.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum WritersVerdict {
    /// The delta can be judged. A cell that rotated maps to its writers at the delta's cut;
    /// every other cell stands at the set its id commits to, which storage holds.
    Judged(BTreeMap<Id, Writers>),
    /// Not yet: the cut cannot be read here, and more governance may clear it.
    Defer(&'static str),
    /// Never: nothing arriving later changes the answer.
    Refuse(&'static str),
}

const NO_POSITION: &str = "the delta cites no governance position but a cell it writes has rotated";
const STALE_POSITION: &str =
    "the delta's governance position is older than one its own parents cited";
const OVER_BUDGET: &str = "a cell it writes has more rotations than the fold takes";
const CUT_UNAVAILABLE: &str = "the governance cut the delta cites cannot be read yet";

/// What the judgement reads of governance.
pub(crate) trait GovernanceCuts {
    /// Whether the context is in a group. A context in none has nothing that can rotate.
    fn in_group(&self) -> bool;
    /// The governance heads this node holds now; `None` when they cannot be read.
    fn current_heads(&self) -> Option<Vec<[u8; 32]>>;
    /// The writers of `cell` at the cut `heads`.
    fn writers_at(&self, cell: Id, heads: &[[u8; 32]]) -> Result<CellWriters, WritersUnavailable>;
    /// Whether the cut `heads` is `frontier` or a descendant of it; `None` when unreadable.
    fn covers(&self, heads: &[[u8; 32]], frontier: &[[u8; 32]]) -> Option<bool>;
}

/// The cells `actions` write: a `Shared` cell by its own id, a `SharedMember` by its anchor's.
pub(crate) fn shared_anchors(actions: &[Action]) -> BTreeSet<Id> {
    actions
        .iter()
        .filter_map(|action| {
            let (Action::Add { metadata, .. }
            | Action::Update { metadata, .. }
            | Action::DeleteRef { metadata, .. }) = action;
            match metadata.storage_type {
                StorageType::Shared { .. } => Some(action.id()),
                StorageType::SharedMember { anchor, .. } => Some(anchor),
                StorageType::Public | StorageType::User { .. } | StorageType::Frozen => None,
            }
        })
        .collect()
}

/// `writers` (by anchor) keyed the way storage looks them up: a `Shared` action by its own id,
/// a `SharedMember` action by its own id too, holding its anchor's set.
pub(crate) fn keyed_by_action(
    actions: &[Action],
    writers: &BTreeMap<Id, Writers>,
) -> BTreeMap<Id, Writers> {
    actions
        .iter()
        .filter_map(|action| {
            let (Action::Add { metadata, .. }
            | Action::Update { metadata, .. }
            | Action::DeleteRef { metadata, .. }) = action;
            let anchor = match metadata.storage_type {
                StorageType::Shared { .. } => action.id(),
                StorageType::SharedMember { anchor, .. } => anchor,
                StorageType::Public | StorageType::User { .. } | StorageType::Frozen => {
                    return None
                }
            };
            writers.get(&anchor).map(|set| (action.id(), set.clone()))
        })
        .collect()
}

/// Judge the cells `anchors` of a delta signed at `position`.
///
/// `parent_positions` is asked only when a cell has rotated, so a delta that touches no shared
/// cell, or only cells that never rotated, reads no parent and folds nothing it has no need of.
pub(crate) fn judge(
    cuts: &dyn GovernanceCuts,
    anchors: &BTreeSet<Id>,
    position: Option<&[[u8; 32]]>,
    parent_positions: impl FnOnce() -> Vec<Vec<[u8; 32]>>,
) -> WritersVerdict {
    if anchors.is_empty() || !cuts.in_group() {
        return WritersVerdict::Judged(BTreeMap::new());
    }
    let Some(cut) = position.filter(|heads| !heads.is_empty()) else {
        // No usable position: only a cell that never rotated can be judged without one.
        let Some(current) = cuts.current_heads() else {
            return WritersVerdict::Defer(CUT_UNAVAILABLE);
        };
        return match any_rotated(cuts, anchors, &current) {
            Ok(false) => WritersVerdict::Judged(BTreeMap::new()),
            Ok(true) => WritersVerdict::Refuse(NO_POSITION),
            Err(verdict) => verdict,
        };
    };

    let mut writers = BTreeMap::new();
    for anchor in anchors {
        match cuts.writers_at(*anchor, cut) {
            Ok(CellWriters::Genesis) => {}
            Ok(CellWriters::Rotated(set)) => {
                let _previous = writers.insert(*anchor, set);
            }
            Err(unavailable) => return unavailable_verdict(unavailable),
        }
    }

    let rotated_later = if writers.is_empty() {
        // Only a cut behind this node's heads can have a rotation still to come.
        let Some(current) = cuts.current_heads() else {
            return WritersVerdict::Defer(CUT_UNAVAILABLE);
        };
        if same_cut(cut, &current) {
            false
        } else {
            match any_rotated(cuts, anchors, &current) {
                Ok(rotated) => rotated,
                Err(verdict) => return verdict,
            }
        }
    } else {
        false
    };
    if !writers.is_empty() || rotated_later {
        for earlier in parent_positions() {
            match cuts.covers(cut, &earlier) {
                Some(true) => {}
                Some(false) => return WritersVerdict::Refuse(STALE_POSITION),
                None => return WritersVerdict::Defer(CUT_UNAVAILABLE),
            }
        }
    }
    WritersVerdict::Judged(writers)
}

/// Whether any of `anchors` has rotated at the cut `heads`. An empty cut has no governance op.
fn any_rotated(
    cuts: &dyn GovernanceCuts,
    anchors: &BTreeSet<Id>,
    heads: &[[u8; 32]],
) -> Result<bool, WritersVerdict> {
    if heads.is_empty() {
        return Ok(false);
    }
    for anchor in anchors {
        match cuts.writers_at(*anchor, heads) {
            Ok(CellWriters::Genesis) => {}
            Ok(CellWriters::Rotated(_)) => return Ok(true),
            Err(unavailable) => return Err(unavailable_verdict(unavailable)),
        }
    }
    Ok(false)
}

fn unavailable_verdict(unavailable: WritersUnavailable) -> WritersVerdict {
    match unavailable {
        WritersUnavailable::Cut => WritersVerdict::Defer(CUT_UNAVAILABLE),
        WritersUnavailable::OverBudget => WritersVerdict::Refuse(OVER_BUDGET),
    }
}

fn same_cut(a: &[[u8; 32]], b: &[[u8; 32]]) -> bool {
    let (mut a, mut b) = (a.to_vec(), b.to_vec());
    a.sort_unstable();
    b.sort_unstable();
    a == b
}

/// Governance as this node's maintained projection holds it, folded up to each cut asked.
pub(crate) struct ProjectionCuts {
    projections: Arc<RwLock<ScopeProjections>>,
    store: Store,
    context_id: ContextId,
    group: Option<ContextGroupId>,
}

impl ProjectionCuts {
    /// # Errors
    /// The store cannot say which group the context is in.
    pub(crate) fn new(
        projections: Arc<RwLock<ScopeProjections>>,
        store: Store,
        context_id: ContextId,
    ) -> eyre::Result<Self> {
        let group = calimero_governance_store::get_group_for_context(&store, &context_id)?;
        Ok(Self {
            projections,
            store,
            context_id,
            group,
        })
    }

    fn folded_to(&self, heads: &[[u8; 32]]) -> Option<ContextGroupId> {
        let group = self.group?;
        ScopeProjections::refresh_for_cut(&self.projections, &self.store, group, heads);
        Some(group)
    }
}

impl GovernanceCuts for ProjectionCuts {
    fn in_group(&self) -> bool {
        self.group.is_some()
    }

    fn current_heads(&self) -> Option<Vec<[u8; 32]>> {
        ScopeProjections::namespace_current_heads(&self.store, self.group?)
    }

    fn writers_at(&self, cell: Id, heads: &[[u8; 32]]) -> Result<CellWriters, WritersUnavailable> {
        self.folded_to(heads).ok_or(WritersUnavailable::Cut)?;
        let projections = self
            .projections
            .read()
            .map_err(|_| WritersUnavailable::Cut)?;
        let answer = projections.shared_writers_at_cut(&self.store, &self.context_id, cell, heads);
        if let Err(unavailable) = &answer {
            debug!(context_id = %self.context_id, ?unavailable, "a cell's writers cannot be read at the delta's cut");
        }
        answer
    }

    fn covers(&self, heads: &[[u8; 32]], frontier: &[[u8; 32]]) -> Option<bool> {
        let group = self.folded_to(heads)?;
        let projections = self.projections.read().ok()?;
        Some(projections.group_cut_covers(&self.store, group, heads, frontier))
    }
}

/// Cells' writers at this node's current governance heads, for a path with no cut of its own
/// (a repair, a pushed leaf). A cell it cannot read has no writers, so the write is refused
/// and a later sync retries it.
pub(crate) struct ProjectionWriters {
    projections: Arc<RwLock<ScopeProjections>>,
    store: Store,
    /// Answers by the heads they were read at, so a session that pushes many leaves of one
    /// cell folds it once. Emptied when it fills, which only costs a fold.
    answered: Mutex<HashMap<AnsweredAt, Option<CellWriters>>>,
}

/// The context, the governance heads and the cell an answer was read for.
type AnsweredAt = (ContextId, Vec<[u8; 32]>, Id);

/// Answers kept before the cache starts over.
const MAX_ANSWERED: usize = 4_096;

impl ProjectionWriters {
    pub(crate) fn new(projections: Arc<RwLock<ScopeProjections>>, store: Store) -> Self {
        Self {
            projections,
            store,
            answered: Mutex::default(),
        }
    }
}

impl CurrentCellWriters for ProjectionWriters {
    fn writers(&self, context_id: &ContextId, cell: Id) -> Option<CellWriters> {
        let cuts = ProjectionCuts::new(
            Arc::clone(&self.projections),
            self.store.clone(),
            *context_id,
        )
        .ok()?;
        if !cuts.in_group() {
            return Some(CellWriters::Genesis);
        }
        let heads = cuts.current_heads()?;
        if heads.is_empty() {
            return Some(CellWriters::Genesis);
        }
        let key = (*context_id, heads, cell);
        if let Some(known) = self
            .answered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return known.clone();
        }
        let answer = cuts.writers_at(cell, &key.1).ok();
        let mut answered = self.answered.lock().unwrap_or_else(|e| e.into_inner());
        if answered.len() >= MAX_ANSWERED {
            answered.clear();
        }
        let _previous = answered.insert(key, answer.clone());
        answer
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use calimero_account::AccountId;
    use calimero_context_client::client::CurrentCellWriters;
    use calimero_storage::entities::OpMask;

    use super::*;

    fn head(n: u8) -> [u8; 32] {
        [n; 32]
    }

    fn cell(n: u8) -> Id {
        Id::new([n; 32])
    }

    fn writers(account: u8) -> Writers {
        [(AccountId::from([account; 32]), OpMask::FULL)].into()
    }

    /// Governance scripted by what a cut answers, with the past of each head.
    struct Scripted {
        in_group: bool,
        current: Option<Vec<[u8; 32]>>,
        /// `(cell, head)` to the answer at the cut made of that one head.
        answers: HashMap<(Id, [u8; 32]), Result<CellWriters, WritersUnavailable>>,
        /// `head` to the heads it descends from.
        past: HashMap<[u8; 32], Vec<[u8; 32]>>,
        asked: RefCell<Vec<(Id, Vec<[u8; 32]>)>>,
    }

    impl Scripted {
        fn new(current: &[[u8; 32]]) -> Self {
            Self {
                in_group: true,
                current: Some(current.to_vec()),
                answers: HashMap::new(),
                past: HashMap::new(),
                asked: RefCell::default(),
            }
        }

        fn answer(
            mut self,
            cell: Id,
            head: [u8; 32],
            answer: Result<CellWriters, WritersUnavailable>,
        ) -> Self {
            let _previous = self.answers.insert((cell, head), answer);
            self
        }

        fn descends(mut self, head: [u8; 32], from: &[[u8; 32]]) -> Self {
            let _previous = self.past.insert(head, from.to_vec());
            self
        }

        fn folds(&self) -> usize {
            self.asked.borrow().len()
        }
    }

    impl GovernanceCuts for Scripted {
        fn in_group(&self) -> bool {
            self.in_group
        }

        fn current_heads(&self) -> Option<Vec<[u8; 32]>> {
            self.current.clone()
        }

        fn writers_at(
            &self,
            cell: Id,
            heads: &[[u8; 32]],
        ) -> Result<CellWriters, WritersUnavailable> {
            self.asked.borrow_mut().push((cell, heads.to_vec()));
            self.answers
                .get(&(cell, heads[0]))
                .cloned()
                .unwrap_or(Ok(CellWriters::Genesis))
        }

        fn covers(&self, heads: &[[u8; 32]], frontier: &[[u8; 32]]) -> Option<bool> {
            Some(frontier.iter().all(|wanted| {
                heads.iter().any(|head| {
                    head == wanted || self.past.get(head).is_some_and(|p| p.contains(wanted))
                })
            }))
        }
    }

    fn anchors(cells: &[Id]) -> BTreeSet<Id> {
        cells.iter().copied().collect()
    }

    fn no_parents() -> Vec<Vec<[u8; 32]>> {
        Vec::new()
    }

    #[test]
    fn a_delta_that_touches_no_shared_cell_folds_nothing() {
        let cuts = Scripted::new(&[head(2)]);
        let verdict = judge(&cuts, &anchors(&[]), Some(&[head(1)]), || {
            unreachable!("no parent is read")
        });
        assert_eq!(verdict, WritersVerdict::Judged(BTreeMap::new()));
        assert_eq!(cuts.folds(), 0);
    }

    #[test]
    fn a_context_in_no_group_stands_at_genesis() {
        let mut cuts = Scripted::new(&[]);
        cuts.in_group = false;
        let verdict = judge(&cuts, &anchors(&[cell(1)]), None, no_parents);
        assert_eq!(verdict, WritersVerdict::Judged(BTreeMap::new()));
        assert_eq!(cuts.folds(), 0);
    }

    #[test]
    fn a_rotated_cell_is_judged_by_the_set_at_the_deltas_own_cut() {
        let cuts = Scripted::new(&[head(2)])
            .answer(cell(1), head(1), Ok(CellWriters::Genesis))
            .answer(cell(1), head(2), Ok(CellWriters::Rotated(writers(9))));
        let before = judge(&cuts, &anchors(&[cell(1)]), Some(&[head(1)]), no_parents);
        assert_eq!(
            before,
            WritersVerdict::Judged(BTreeMap::new()),
            "signed before the rotation: the set the cell id commits to"
        );
        let after = judge(&cuts, &anchors(&[cell(1)]), Some(&[head(2)]), no_parents);
        assert_eq!(
            after,
            WritersVerdict::Judged([(cell(1), writers(9))].into()),
            "signed after it: the rotated set"
        );
    }

    #[test]
    fn a_cut_that_cannot_be_read_defers_the_delta() {
        let cuts = Scripted::new(&[head(2)]).answer(cell(1), head(1), Err(WritersUnavailable::Cut));
        let verdict = judge(&cuts, &anchors(&[cell(1)]), Some(&[head(1)]), no_parents);
        assert_eq!(verdict, WritersVerdict::Defer(CUT_UNAVAILABLE));
    }

    #[test]
    fn a_cell_over_the_rotation_budget_is_refused() {
        let cuts =
            Scripted::new(&[head(2)]).answer(cell(1), head(1), Err(WritersUnavailable::OverBudget));
        let verdict = judge(&cuts, &anchors(&[cell(1)]), Some(&[head(1)]), no_parents);
        assert_eq!(verdict, WritersVerdict::Refuse(OVER_BUDGET));
    }

    #[test]
    fn a_delta_without_a_position_is_refused_once_a_cell_has_rotated() {
        let cuts = Scripted::new(&[head(2)]).answer(
            cell(1),
            head(2),
            Ok(CellWriters::Rotated(writers(9))),
        );
        for position in [None, Some(&[][..])] {
            let verdict = judge(&cuts, &anchors(&[cell(1)]), position, no_parents);
            assert_eq!(verdict, WritersVerdict::Refuse(NO_POSITION), "{position:?}");
        }
    }

    #[test]
    fn a_delta_without_a_position_is_judged_while_no_cell_has_rotated() {
        let cuts = Scripted::new(&[head(2)]);
        let verdict = judge(&cuts, &anchors(&[cell(1)]), None, no_parents);
        assert_eq!(verdict, WritersVerdict::Judged(BTreeMap::new()));
        let none_yet = Scripted::new(&[]);
        let verdict = judge(&none_yet, &anchors(&[cell(1)]), None, no_parents);
        assert_eq!(verdict, WritersVerdict::Judged(BTreeMap::new()));
        assert_eq!(none_yet.folds(), 0, "no governance op, nothing to fold");
    }

    #[test]
    fn a_position_older_than_a_parents_is_refused_once_the_cell_rotated() {
        // The removed writer signs at head 1, before the rotation at head 2, but builds on a
        // parent that had already cited head 2.
        let cuts = Scripted::new(&[head(2)])
            .descends(head(2), &[head(1)])
            .answer(cell(1), head(2), Ok(CellWriters::Rotated(writers(9))));
        let stale = judge(&cuts, &anchors(&[cell(1)]), Some(&[head(1)]), || {
            vec![vec![head(2)]]
        });
        assert_eq!(stale, WritersVerdict::Refuse(STALE_POSITION));

        let honest = judge(&cuts, &anchors(&[cell(1)]), Some(&[head(1)]), || {
            vec![vec![head(1)]]
        });
        assert_eq!(
            honest,
            WritersVerdict::Judged(BTreeMap::new()),
            "a pre-rotation write whose parents cited no more is still accepted"
        );

        let descendant = judge(&cuts, &anchors(&[cell(1)]), Some(&[head(2)]), || {
            vec![vec![head(1)], vec![head(2)]]
        });
        assert_eq!(
            descendant,
            WritersVerdict::Judged([(cell(1), writers(9))].into()),
            "a position that descends from every parent's is fresh"
        );
    }

    #[test]
    fn parents_are_not_read_while_no_cell_has_rotated() {
        let cuts = Scripted::new(&[head(2)]);
        let verdict = judge(&cuts, &anchors(&[cell(1)]), Some(&[head(1)]), || {
            unreachable!("a cell that never rotated has no stale position to worry about")
        });
        assert_eq!(verdict, WritersVerdict::Judged(BTreeMap::new()));
    }

    #[test]
    fn a_delta_at_the_current_cut_is_folded_once_per_cell() {
        let cuts = Scripted::new(&[head(2)]);
        let _verdict = judge(
            &cuts,
            &anchors(&[cell(1), cell(2)]),
            Some(&[head(2)]),
            no_parents,
        );
        assert_eq!(cuts.folds(), 2);
    }

    #[test]
    fn a_rotation_that_fell_after_the_cut_still_checks_the_parents() {
        // Genesis at the deltas cut, rotated at the node's: the forger's case.
        let cuts = Scripted::new(&[head(2)])
            .descends(head(2), &[head(1)])
            .answer(cell(1), head(2), Ok(CellWriters::Rotated(writers(9))));
        let verdict = judge(&cuts, &anchors(&[cell(1)]), Some(&[head(1)]), || {
            vec![vec![head(2)]]
        });
        assert_eq!(verdict, WritersVerdict::Refuse(STALE_POSITION));
    }

    #[test]
    fn heads_that_cannot_be_read_defer_a_delta_that_needs_them() {
        let mut cuts = Scripted::new(&[]);
        cuts.current = None;
        // Without a position the current heads are all there is to judge by.
        let verdict = judge(&cuts, &anchors(&[cell(1)]), None, no_parents);
        assert_eq!(verdict, WritersVerdict::Defer(CUT_UNAVAILABLE));
        // With one at genesis they say whether the cell has rotated since.
        let verdict = judge(&cuts, &anchors(&[cell(1)]), Some(&[head(1)]), no_parents);
        assert_eq!(verdict, WritersVerdict::Defer(CUT_UNAVAILABLE));
        // A rotated cell is already known to have rotated, so they add nothing.
        let cuts = Scripted {
            current: None,
            ..Scripted::new(&[])
        }
        .answer(cell(1), head(1), Ok(CellWriters::Rotated(writers(9))));
        let verdict = judge(&cuts, &anchors(&[cell(1)]), Some(&[head(1)]), no_parents);
        assert_eq!(
            verdict,
            WritersVerdict::Judged([(cell(1), writers(9))].into())
        );
    }

    #[test]
    fn a_current_cut_that_cannot_be_read_defers_a_position_less_delta() {
        let cuts = Scripted::new(&[head(2)]).answer(cell(1), head(2), Err(WritersUnavailable::Cut));
        let verdict = judge(&cuts, &anchors(&[cell(1)]), None, no_parents);
        assert_eq!(verdict, WritersVerdict::Defer(CUT_UNAVAILABLE));
    }

    /// The governance fold as production reads it: real ops in a real projection.
    mod over_the_fold {
        use calimero_context::test_support::RotationWorld;
        use calimero_primitives::identity::PublicKey;
        use calimero_storage::tests::common::cell_at;

        use super::*;

        const ROTATION: [u8; 32] = [0xD1; 32];

        struct World {
            world: RotationWorld,
            cell: Id,
            genesis: Writers,
            rotated: Writers,
            cuts: ProjectionCuts,
        }

        fn world() -> World {
            let (alice, bob) = (PublicKey::from([1; 32]), PublicKey::from([2; 32]));
            let world = RotationWorld::new(&[alice, bob]);
            let (a, b) = (world.account(&alice), world.account(&bob));
            let genesis = calimero_storage::entities::full_mask([a, b].into_iter().collect());
            let rotated = calimero_storage::entities::full_mask([a].into_iter().collect());
            let cell = cell_at(0x40, &[a, b].into_iter().collect());
            world.rotate(
                &alice,
                cell,
                ROTATION,
                &world.joined(),
                genesis.clone(),
                1,
                rotated.clone(),
            );
            world.set_current_heads(&[ROTATION]);
            let cuts = ProjectionCuts::new(
                Arc::clone(&world.projections),
                world.store.clone(),
                world.context,
            )
            .expect("the group is readable");
            World {
                world,
                cell,
                genesis,
                rotated,
                cuts,
            }
        }

        #[test]
        fn a_cell_reads_as_rotated_only_from_the_rotation_on() {
            let w = world();
            assert!(w.cuts.in_group());
            assert_eq!(
                w.cuts.writers_at(w.cell, &w.world.joined()),
                Ok(CellWriters::Genesis)
            );
            assert_eq!(
                w.cuts.writers_at(w.cell, &[ROTATION]),
                Ok(CellWriters::Rotated(w.rotated.clone()))
            );
            assert_eq!(w.cuts.current_heads(), Some(vec![ROTATION]));
            let _ = &w.genesis;
        }

        #[test]
        fn an_unfolded_cut_cannot_be_read() {
            let w = world();
            assert_eq!(
                w.cuts.writers_at(w.cell, &[[0xEE; 32]]),
                Err(WritersUnavailable::Cut)
            );
            assert_eq!(w.cuts.covers(&[[0xEE; 32]], &w.world.joined()), Some(false));
        }

        #[test]
        fn a_cut_covers_the_cuts_behind_it_and_not_the_ones_ahead() {
            let w = world();
            let joined = w.world.joined();
            assert_eq!(w.cuts.covers(&[ROTATION], &joined), Some(true));
            assert_eq!(w.cuts.covers(&[ROTATION], &[ROTATION]), Some(true));
            assert_eq!(w.cuts.covers(&joined, &[ROTATION]), Some(false));
        }

        #[test]
        fn a_repair_reads_the_writers_at_the_nodes_current_heads() {
            let w = world();
            let at_heads =
                ProjectionWriters::new(Arc::clone(&w.world.projections), w.world.store.clone());
            assert_eq!(
                at_heads.writers(&w.world.context, w.cell),
                Some(CellWriters::Rotated(w.rotated.clone()))
            );
            w.world.set_current_heads(&w.world.joined());
            assert_eq!(
                at_heads.writers(&w.world.context, w.cell),
                Some(CellWriters::Genesis),
                "an answer is kept only for the heads it was read at"
            );
            assert_eq!(
                at_heads.writers(&ContextId::from([0x99; 32]), w.cell),
                Some(CellWriters::Genesis),
                "a context in no group has nothing that can rotate"
            );
        }
    }
}
