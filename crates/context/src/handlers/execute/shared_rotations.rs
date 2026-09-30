//! A run reads a cell's writers from the governance fold at a cut pinned before it, and
//! its rotation requests are published as governance ops before its delta is finalised.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, RwLock};

use calimero_account::AccountId;
use calimero_context_client::local_governance::{AckRouter, GroupOp};
use calimero_context_client::messages::{ExecuteError, SharedRotationRefusal};
use calimero_context_config::types::{ContextGroupId, GovernanceParentEdge};
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::NamespaceRepository;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PrivateKey;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::collections::cell_id_binds;
use calimero_storage::delta::StorageDelta;
use calimero_storage::shared_writers::{
    cell_uses, shared_anchors, CellWriters, SharedRotation, Writers,
};
use calimero_store::Store;
use tracing::debug;

use super::storage::SharedWritersResolver;
use crate::error::ContextError;
use crate::scope_projection::ScopeProjections;

/// Who a run acts for, which decides whether it may rotate a cell.
#[derive(Clone, Copy, Debug)]
pub(super) struct RunKind {
    /// On someone else's behalf, or a read for another account.
    pub delegated: bool,
    /// As a TEE authority.
    pub tee: bool,
    /// The merge-apply of a peer's delta.
    pub state_op: bool,
}

impl RunKind {
    /// Why this run may not rotate, if it may not.
    fn refusal(self) -> Option<SharedRotationRefusal> {
        if self.delegated {
            Some(SharedRotationRefusal::Delegated)
        } else if self.tee {
            Some(SharedRotationRefusal::Tee)
        } else if self.state_op {
            Some(SharedRotationRefusal::StateOp)
        } else {
            None
        }
    }
}

/// The ops that publish `rotations` in order, each stepping from the set in effect: the
/// fold's, or at genesis the run's own if the cell id commits to it, or the run's last.
fn plan_rotation_ops(
    context_id: ContextId,
    rotations: &[SharedRotation],
    mut writers_at_cut: impl FnMut(Id) -> Option<CellWriters>,
    first_nonce: u64,
) -> Result<Vec<GroupOp>, SharedRotationRefusal> {
    // The set each cell was last rotated to in this run.
    let mut stepped_to: BTreeMap<Id, &Writers> = BTreeMap::new();
    let mut ops = Vec::with_capacity(rotations.len());
    for (nonce, rotation) in (first_nonce..).zip(rotations) {
        let prior = match stepped_to.get(&rotation.cell) {
            Some(previous) => (*previous).clone(),
            None => match writers_at_cut(rotation.cell)
                .ok_or(SharedRotationRefusal::WritersUnavailable)?
            {
                CellWriters::Rotated(in_effect) => in_effect,
                CellWriters::Genesis if cell_id_binds(rotation.cell, &rotation.prior) => {
                    rotation.prior.clone()
                }
                CellWriters::Genesis => return Err(SharedRotationRefusal::PriorNotBound),
            },
        };
        let _previous = stepped_to.insert(rotation.cell, &rotation.new);
        ops.push(GroupOp::SharedWritersRotated {
            context_id,
            cell: rotation.cell,
            prior,
            nonce,
            new: rotation.new.clone(),
        });
    }
    Ok(ops)
}

/// The set each rotated cell is left in, by its last rotation in the run.
fn final_sets(rotations: &[SharedRotation]) -> BTreeMap<Id, &Writers> {
    rotations.iter().map(|r| (r.cell, &r.new)).collect()
}

/// Refuses a run that writes a cell it rotates when the set it leaves `author` without what the
/// run did there: `WRITE` (or `WRITE_ONCE`) to put, `DELETE` to delete, and for an owned entry
/// its owner's `WRITE`. Receivers judge the delta at a position that includes the rotation and
/// would refuse the write.
fn check_author_keeps_rights(
    rotations: &[SharedRotation],
    author: &AccountId,
    actions: &[Action],
) -> Result<(), SharedRotationRefusal> {
    let sets = final_sets(rotations);
    let cells: BTreeSet<Id> = sets.keys().copied().collect();
    let loses_rights = cell_uses(actions, &cells).into_iter().any(|cell_use| {
        let holder = cell_use.account.unwrap_or(*author);
        !sets[&cell_use.cell]
            .get(&holder)
            .is_some_and(|mask| cell_use.op.granted_by(*mask))
    });
    if loses_rights {
        return Err(SharedRotationRefusal::RemovesOwnWrite);
    }
    Ok(())
}

/// The actions of a run's artifact.
fn run_actions(artifact: &[u8]) -> Vec<Action> {
    match borsh::from_slice::<StorageDelta>(artifact) {
        Ok(StorageDelta::Actions(actions)) => actions,
        _ => Vec::new(),
    }
}

/// Milliseconds since the epoch, for a nonce that grows from one rotation to the next.
fn now_millis() -> eyre::Result<u64> {
    let elapsed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    Ok(u64::try_from(elapsed.as_millis())?)
}

/// Fails a run that recorded rotations where none can be published: a migration or a
/// context's first run, which have no governance cut of their own.
pub(crate) fn refuse_unpublishable(
    context_id: ContextId,
    rotations: &[SharedRotation],
) -> eyre::Result<()> {
    if rotations.is_empty() {
        return Ok(());
    }
    Err(eyre::Report::new(ExecuteError::SharedRotationRefused {
        context_id,
        reason: SharedRotationRefusal::Unpublishable,
    }))
}

/// The cut a run reads its cells' writers at.
pub(super) struct PinnedCut {
    /// The context's group; a context in no group has nothing that can have rotated.
    pub group_id: Option<ContextGroupId>,
    /// The governance heads the cut names.
    pub heads: Vec<[u8; 32]>,
    /// A resolver of a cell's writers at the cut.
    pub writers: SharedWritersResolver,
}

/// The cut of a run: the heads a state op's position names or else the current heads, with the
/// fold brought up to them.
pub(super) fn pin_cut(
    store: &Store,
    projections: &Arc<RwLock<ScopeProjections>>,
    context_id: ContextId,
    position: Option<&GovernanceParentEdge>,
) -> eyre::Result<PinnedCut> {
    let Some(group) = calimero_governance_store::get_group_for_context(store, &context_id)? else {
        return Ok(PinnedCut {
            group_id: None,
            heads: Vec::new(),
            writers: Arc::new(|_| Some(CellWriters::Genesis)),
        });
    };
    let heads = match position.map(|edge| edge.governance_dag_heads.clone()) {
        Some(heads) if !heads.is_empty() => heads,
        _ => ScopeProjections::namespace_current_heads(store, group).unwrap_or_default(),
    };
    ScopeProjections::refresh_for_cut(projections, store, group, &heads);
    let (reader, projections) = (store.clone(), Arc::clone(projections));
    let writers = memoized({
        let heads = heads.clone();
        move |cell| writers_at_cut(&projections, &reader, &context_id, *cell, &heads)
    });
    Ok(PinnedCut {
        group_id: Some(group),
        heads,
        writers,
    })
}

/// Asks `ask` once per cell: the cut is fixed for the run and the guest reads on every access.
fn memoized(
    ask: impl Fn(&[u8; 32]) -> Option<CellWriters> + Send + Sync + 'static,
) -> SharedWritersResolver {
    let answered: Mutex<HashMap<[u8; 32], Option<CellWriters>>> = Mutex::new(HashMap::new());
    Arc::new(move |cell| {
        if let Some(known) = answered
            .lock()
            .ok()
            .and_then(|seen| seen.get(cell).cloned())
        {
            return known;
        }
        let answer = ask(cell);
        if let Ok(mut seen) = answered.lock() {
            let _prior = seen.insert(*cell, answer.clone());
        }
        answer
    })
}

/// The fold's answer for `cell` at `heads`; `None` when it gives none, poisoned lock included.
fn writers_at_cut(
    projections: &RwLock<ScopeProjections>,
    store: &Store,
    context_id: &ContextId,
    cell: [u8; 32],
    heads: &[[u8; 32]],
) -> Option<CellWriters> {
    let projections = projections.read().ok()?;
    match projections.shared_writers_at_cut(store, context_id, Id::new(cell), heads) {
        Ok(writers) => Some(writers),
        Err(unavailable) => {
            debug!(%context_id, ?unavailable, "a cell's writers cannot be read at the run's cut");
            None
        }
    }
}

/// Where a run's rotations are published.
pub(super) struct Publisher<'a> {
    pub store: &'a Store,
    pub node_client: &'a NodeClient,
    pub ack_router: &'a AckRouter,
    pub projections: &'a Arc<RwLock<ScopeProjections>>,
    pub context_id: ContextId,
    pub group_id: Option<ContextGroupId>,
    /// The account the run writes for.
    pub author: AccountId,
}

impl Publisher<'_> {
    /// Publish `rotations` signed by this node so the run's delta, whose actions are `artifact`,
    /// cites them. Nothing goes out unless all are admissible; ops published before a failing
    /// one stay published. Fails unless each cell then reads back as the run left it.
    pub(super) async fn publish(
        &self,
        run: RunKind,
        rotations: &[SharedRotation],
        artifact: &[u8],
        resolver: &SharedWritersResolver,
    ) -> eyre::Result<()> {
        let refuse = |reason| {
            eyre::Report::new(ExecuteError::SharedRotationRefused {
                context_id: self.context_id,
                reason,
            })
        };
        if let Some(reason) = run.refusal() {
            return Err(refuse(reason));
        }
        let group_id = self
            .group_id
            .ok_or_else(|| refuse(SharedRotationRefusal::NoGroup))?;
        check_author_keeps_rights(rotations, &self.author, &run_actions(artifact))
            .map_err(refuse)?;
        let ops = plan_rotation_ops(
            self.context_id,
            rotations,
            |cell| resolver(cell.as_bytes()),
            now_millis()?,
        )
        .map_err(refuse)?;

        // The node signs as itself, as `delete_context` does.
        let Some((_public, secret)) =
            NamespaceRepository::new(self.store).resolve_identity(&group_id)?
        else {
            eyre::bail!(ContextError::NotAGroupMember {
                group_id: group_id.to_string(),
            });
        };
        let signer = PrivateKey::from(secret);
        for op in ops {
            let report = calimero_governance_store::sign_apply_and_publish(
                self.store,
                self.node_client,
                self.ack_router,
                &group_id,
                &signer,
                op,
            )
            .await?
            .ok_or_else(|| {
                eyre::eyre!("writers rotation applied here but not published: no namespace identity or group key")
            })?;
            report.observe("execute", "SharedWritersRotated");
        }

        // The publisher bypasses the apply feed, so fold what it wrote now for the next run.
        let heads = ScopeProjections::namespace_current_heads(self.store, group_id)
            .ok_or_else(|| refuse(SharedRotationRefusal::WritersUnavailable))?;
        ScopeProjections::refresh_for_cut(self.projections, self.store, group_id, &heads);
        // A step built on a set another admin has since changed applies without effect.
        for (cell, new) in final_sets(rotations) {
            let read = writers_at_cut(
                self.projections,
                self.store,
                &self.context_id,
                *cell.as_bytes(),
                &heads,
            );
            match read {
                Some(CellWriters::Rotated(in_effect)) if in_effect == *new => {}
                Some(_) => return Err(refuse(SharedRotationRefusal::NotApplied)),
                None => return Err(refuse(SharedRotationRefusal::WritersUnavailable)),
            }
        }
        Ok(())
    }

    /// Fails a run whose author, at the heads its delta is signed at (`position`), no longer holds
    /// what the run did to a cell it wrote, so its writes are dropped rather than forked from
    /// peers that judge the delta there. Reads nothing when those heads are the ones the run was
    /// pinned at. A run that wrote a cell and has no position fails closed.
    pub(super) fn verify_signing_cut(
        &self,
        pinned: &PinnedCut,
        position: Option<&GovernanceParentEdge>,
        rotations: &[SharedRotation],
        artifact: &[u8],
    ) -> eyre::Result<()> {
        let refuse = |reason| {
            eyre::Report::new(ExecuteError::SharedRotationRefused {
                context_id: self.context_id,
                reason,
            })
        };
        let Some(group_id) = self.group_id else {
            return Ok(());
        };
        let actions = run_actions(artifact);
        let mut cells = shared_anchors(&actions);
        cells.extend(rotations.iter().map(|rotation| rotation.cell));
        let uses = cell_uses(&actions, &cells);
        if uses.is_empty() {
            return Ok(());
        }
        let Some(position) = position else {
            return Err(refuse(SharedRotationRefusal::WritersUnavailable));
        };
        let mut heads = position.governance_dag_heads.clone();
        heads.sort_unstable();
        let mut pinned_heads = pinned.heads.clone();
        pinned_heads.sort_unstable();
        if heads == pinned_heads {
            return Ok(());
        }
        ScopeProjections::refresh_for_cut(self.projections, self.store, group_id, &heads);

        // `None` for a cell nothing rotated at either cut: the set that stood still does.
        let mut at_signing: BTreeMap<Id, Option<Writers>> = BTreeMap::new();
        for cell in uses.iter().map(|cell_use| cell_use.cell) {
            if at_signing.contains_key(&cell) {
                continue;
            }
            let read = writers_at_cut(
                self.projections,
                self.store,
                &self.context_id,
                *cell.as_bytes(),
                &heads,
            );
            let rights = match read {
                Some(CellWriters::Rotated(rights)) => Some(rights),
                // A rotation in effect at the pinned cut that is void now changed the set.
                Some(CellWriters::Genesis) => match (pinned.writers)(cell.as_bytes()) {
                    Some(CellWriters::Genesis) => None,
                    _ => return Err(refuse(SharedRotationRefusal::NotApplied)),
                },
                None => return Err(refuse(SharedRotationRefusal::WritersUnavailable)),
            };
            let _previous = at_signing.insert(cell, rights);
        }
        for cell_use in uses {
            let Some(Some(rights)) = at_signing.get(&cell_use.cell) else {
                continue;
            };
            let holder = cell_use.account.unwrap_or(self.author);
            if !rights
                .get(&holder)
                .is_some_and(|mask| cell_use.op.granted_by(*mask))
            {
                return Err(refuse(SharedRotationRefusal::RemovesOwnWrite));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use calimero_account::AccountId;
    use calimero_storage::collections::cell_id;
    use calimero_storage::entities::OpMask;
    use calimero_store::db::InMemoryDB;

    use super::*;

    fn writers(accounts: &[u8]) -> Writers {
        accounts
            .iter()
            .map(|&b| (AccountId::from([b; 32]), OpMask::FULL))
            .collect()
    }

    fn cell_of(genesis: &[u8], field: u8) -> Id {
        cell_id(Id::new([field; 32]), &writers(genesis))
    }

    fn rotation(cell: Id, prior: &[u8], new: &[u8]) -> SharedRotation {
        SharedRotation {
            cell,
            prior: writers(prior),
            new: writers(new),
        }
    }

    fn context() -> ContextId {
        ContextId::from([0xC7; 32])
    }

    /// What an op says, since `GroupOp` has no equality: its context, cell, prior set,
    /// nonce and new set.
    type Step = (ContextId, Id, Writers, u64, Writers);

    fn step(cell: Id, prior: &[u8], nonce: u64, new: &[u8]) -> Step {
        (context(), cell, writers(prior), nonce, writers(new))
    }

    fn steps(
        ops: Result<Vec<GroupOp>, SharedRotationRefusal>,
    ) -> Result<Vec<Step>, SharedRotationRefusal> {
        ops.map(|ops| {
            ops.into_iter()
                .map(|op| match op {
                    GroupOp::SharedWritersRotated {
                        context_id,
                        cell,
                        prior,
                        nonce,
                        new,
                    } => (context_id, cell, prior, nonce, new),
                    other => panic!("planned a {}", other.op_kind_label()),
                })
                .collect()
        })
    }

    fn at_genesis(_cell: Id) -> Option<CellWriters> {
        Some(CellWriters::Genesis)
    }

    #[test]
    fn a_cell_at_genesis_steps_from_the_set_its_id_commits_to() {
        let cell = cell_of(&[1], 7);
        let ops = plan_rotation_ops(context(), &[rotation(cell, &[1], &[1, 2])], at_genesis, 100);
        assert_eq!(steps(ops), Ok(vec![step(cell, &[1], 100, &[1, 2])]));
    }

    #[test]
    fn a_cell_at_genesis_is_refused_a_prior_its_id_does_not_commit_to() {
        let cell = cell_of(&[1], 7);
        let ops = plan_rotation_ops(context(), &[rotation(cell, &[1, 9], &[2])], at_genesis, 0);
        assert_eq!(steps(ops), Err(SharedRotationRefusal::PriorNotBound));
    }

    #[test]
    fn a_rotated_cell_steps_from_the_set_in_effect_not_the_one_the_run_claimed() {
        let cell = cell_of(&[1], 7);
        let in_effect = |_| Some(CellWriters::Rotated(writers(&[1, 2])));
        let ops = plan_rotation_ops(context(), &[rotation(cell, &[1], &[2])], in_effect, 5);
        assert_eq!(steps(ops), Ok(vec![step(cell, &[1, 2], 5, &[2])]));
    }

    #[test]
    fn a_later_rotation_of_a_cell_steps_from_the_one_before_it() {
        let cell = cell_of(&[1], 7);
        let other = cell_of(&[3], 8);
        let ops = plan_rotation_ops(
            context(),
            &[
                rotation(cell, &[1], &[1, 2]),
                rotation(other, &[3], &[3, 4]),
                rotation(cell, &[1, 2], &[2]),
            ],
            at_genesis,
            10,
        );
        assert_eq!(
            steps(ops),
            Ok(vec![
                step(cell, &[1], 10, &[1, 2]),
                step(other, &[3], 11, &[3, 4]),
                step(cell, &[1, 2], 12, &[2]),
            ]),
            "nonces grow across the run, and the second step of a cell starts at the first's end"
        );
    }

    #[test]
    fn a_later_rotation_needs_no_binding_of_its_own() {
        let cell = cell_of(&[1], 7);
        let ops = plan_rotation_ops(
            context(),
            &[rotation(cell, &[1], &[1, 2]), rotation(cell, &[8, 9], &[2])],
            at_genesis,
            0,
        );
        assert_eq!(
            steps(ops),
            Ok(vec![
                step(cell, &[1], 0, &[1, 2]),
                step(cell, &[1, 2], 1, &[2]),
            ]),
            "the chain is the node's, whatever the guest named after the first"
        );
    }

    #[test]
    fn the_fold_is_asked_once_per_cell() {
        let cell = cell_of(&[1], 7);
        let mut asked = Vec::new();
        let ops = plan_rotation_ops(
            context(),
            &[rotation(cell, &[1], &[1, 2]), rotation(cell, &[1, 2], &[2])],
            |cell| {
                asked.push(cell);
                Some(CellWriters::Genesis)
            },
            0,
        );
        assert!(ops.is_ok());
        assert_eq!(asked, vec![cell]);
    }

    #[test]
    fn a_cell_the_fold_cannot_answer_for_refuses_the_whole_plan() {
        let known = cell_of(&[1], 7);
        let unknown = cell_of(&[3], 8);
        let ops = plan_rotation_ops(
            context(),
            &[
                rotation(known, &[1], &[1, 2]),
                rotation(unknown, &[3], &[3, 4]),
            ],
            |cell| (cell == known).then_some(CellWriters::Genesis),
            0,
        );
        assert_eq!(steps(ops), Err(SharedRotationRefusal::WritersUnavailable));
    }

    #[test]
    fn no_rotations_plan_no_ops() {
        assert_eq!(
            steps(plan_rotation_ops(context(), &[], |_| None, 0)),
            Ok(Vec::new())
        );
    }

    #[test]
    fn a_direct_run_may_rotate_and_no_other_may() {
        let direct = RunKind {
            delegated: false,
            tee: false,
            state_op: false,
        };
        assert_eq!(direct.refusal(), None);
        assert_eq!(
            RunKind {
                delegated: true,
                ..direct
            }
            .refusal(),
            Some(SharedRotationRefusal::Delegated)
        );
        assert_eq!(
            RunKind {
                tee: true,
                ..direct
            }
            .refusal(),
            Some(SharedRotationRefusal::Tee)
        );
        assert_eq!(
            RunKind {
                state_op: true,
                ..direct
            }
            .refusal(),
            Some(SharedRotationRefusal::StateOp)
        );
    }

    fn account(byte: u8) -> AccountId {
        AccountId::from([byte; 32])
    }

    fn stamped(
        storage_type: calimero_storage::entities::StorageType,
    ) -> calimero_storage::entities::Metadata {
        let mut metadata = calimero_storage::entities::Metadata::new(1, 1);
        metadata.storage_type = storage_type;
        metadata
    }

    /// An update of a member of `cell`.
    fn update_member(cell: Id) -> Action {
        Action::Update {
            id: Id::new([0xCC; 32]),
            data: vec![1],
            ancestors: Vec::new(),
            metadata: stamped(calimero_storage::entities::StorageType::SharedMember {
                anchor: cell,
                signature_data: None,
            }),
        }
    }

    fn delete_member(cell: Id) -> Action {
        Action::DeleteRef {
            id: Id::new([0xCC; 32]),
            deleted_at: 1,
            metadata: stamped(calimero_storage::entities::StorageType::SharedMember {
                anchor: cell,
                signature_data: None,
            }),
        }
    }

    #[test]
    fn a_run_may_rotate_a_cell_it_did_not_write_out_of_its_own_writers() {
        let cell = cell_of(&[1], 7);
        let rotations = [rotation(cell, &[1], &[2])];
        assert_eq!(
            check_author_keeps_rights(&rotations, &account(1), &[]),
            Ok(())
        );
    }

    #[test]
    fn a_run_that_writes_a_cell_must_leave_its_author_able_to_write_it() {
        let cell = cell_of(&[1], 7);
        let rotations = [rotation(cell, &[1], &[2])];
        let wrote = [update_member(cell)];
        assert_eq!(
            check_author_keeps_rights(&rotations, &account(1), &wrote),
            Err(SharedRotationRefusal::RemovesOwnWrite)
        );
        assert_eq!(
            check_author_keeps_rights(&rotations, &account(2), &wrote),
            Ok(()),
            "the new set holds the author"
        );
    }

    #[test]
    fn only_the_set_a_cell_ends_the_run_with_is_judged() {
        let cell = cell_of(&[1], 7);
        let rotations = [rotation(cell, &[1], &[2]), rotation(cell, &[2], &[1, 2])];
        assert_eq!(
            check_author_keeps_rights(&rotations, &account(1), &[update_member(cell)]),
            Ok(())
        );
    }

    #[test]
    fn an_author_who_keeps_only_admin_cannot_write() {
        let cell = cell_of(&[1], 7);
        let mut read_only = rotation(cell, &[1], &[1]);
        let _ = read_only.new.insert(account(1), OpMask::ADMIN);
        assert_eq!(
            check_author_keeps_rights(&[read_only], &account(1), &[update_member(cell)]),
            Err(SharedRotationRefusal::RemovesOwnWrite)
        );
    }

    #[test]
    fn a_delete_needs_delete_and_a_write_does_not() {
        let cell = cell_of(&[1], 7);
        let mut keeps_write = rotation(cell, &[1], &[1]);
        let _ = keeps_write.new.insert(account(1), OpMask::WRITE);
        assert_eq!(
            check_author_keeps_rights(
                std::slice::from_ref(&keeps_write),
                &account(1),
                &[update_member(cell)]
            ),
            Ok(())
        );
        assert_eq!(
            check_author_keeps_rights(&[keeps_write], &account(1), &[delete_member(cell)]),
            Err(SharedRotationRefusal::RemovesOwnWrite)
        );
    }

    #[test]
    fn the_actions_of_a_run_are_read_from_its_artifact() {
        let cell = cell_of(&[1], 7);
        let actions = vec![update_member(cell)];
        let artifact = borsh::to_vec(&StorageDelta::Actions(actions.clone())).expect("encodes");
        assert_eq!(run_actions(&artifact), actions);
        assert_eq!(run_actions(&[]), vec![]);
        assert_eq!(run_actions(&[0xFF; 3]), vec![]);
    }

    #[test]
    fn a_run_that_cannot_publish_fails_when_it_recorded_a_rotation() {
        let cell = cell_of(&[1], 7);
        assert!(refuse_unpublishable(context(), &[]).is_ok());
        let error = refuse_unpublishable(context(), &[rotation(cell, &[1], &[2])])
            .expect_err("a rotation nobody can publish");
        assert!(matches!(
            error.downcast_ref::<ExecuteError>(),
            Some(ExecuteError::SharedRotationRefused {
                reason: SharedRotationRefusal::Unpublishable,
                ..
            })
        ));
    }

    fn empty_projections() -> Arc<RwLock<ScopeProjections>> {
        Arc::new(RwLock::new(ScopeProjections::new()))
    }

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    #[test]
    fn an_empty_cut_fails_closed() {
        let projections = empty_projections();
        assert_eq!(
            writers_at_cut(&projections, &store(), &context(), [7; 32], &[]),
            None
        );
    }

    #[test]
    fn a_cut_the_fold_cannot_read_answers_none() {
        let projections = empty_projections();
        assert_eq!(
            writers_at_cut(&projections, &store(), &context(), [7; 32], &[[1; 32]]),
            None,
            "the context is in no group here, which the fold reports as an error"
        );
    }

    #[test]
    fn a_poisoned_lock_answers_none() {
        let projections = empty_projections();
        let held = Arc::clone(&projections);
        let _ = std::thread::spawn(move || {
            let _guard = held.write().expect("the lock is not poisoned yet");
            panic!("poison the lock");
        })
        .join();
        assert_eq!(
            writers_at_cut(&projections, &store(), &context(), [7; 32], &[[1; 32]]),
            None
        );
    }

    #[test]
    fn a_context_in_no_group_has_every_cell_at_genesis() {
        let pinned = pin_cut(&store(), &empty_projections(), context(), None)
            .expect("an unregistered context pins");
        assert_eq!(pinned.group_id, None);
        assert_eq!((pinned.writers)(&[7; 32]), Some(CellWriters::Genesis));
    }

    #[test]
    fn the_resolver_asks_once_per_cell_and_remembers_a_refusal() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let resolve = {
            let asked = Arc::clone(&asked);
            memoized(move |cell| {
                asked.lock().expect("not poisoned").push(cell[0]);
                (cell[0] == 1).then_some(CellWriters::Genesis)
            })
        };
        for _ in 0..3 {
            assert_eq!(resolve(&[1; 32]), Some(CellWriters::Genesis));
            assert_eq!(resolve(&[2; 32]), None);
        }
        assert_eq!(*asked.lock().expect("not poisoned"), vec![1, 2]);
    }
}
