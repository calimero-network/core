//! A run reads a cell's writers from the governance fold at a cut pinned before it, and
//! its rotation requests are published as governance ops before its delta is finalised.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock};

use calimero_context_client::local_governance::{AckRouter, GroupOp};
use calimero_context_client::messages::{ExecuteError, SharedRotationRefusal};
use calimero_context_config::types::{ContextGroupId, GovernanceParentEdge};
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::NamespaceRepository;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PrivateKey;
use calimero_storage::address::Id;
use calimero_storage::collections::cell_id_binds;
use calimero_storage::shared_writers::{CellWriters, SharedRotation, Writers};
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

/// Milliseconds since the epoch, for a nonce that grows from one rotation to the next.
fn now_millis() -> eyre::Result<u64> {
    let elapsed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    Ok(u64::try_from(elapsed.as_millis())?)
}

/// The context's group and a resolver of its cells' writers at the run's cut, the position
/// a state op carries or else the current heads, with the fold brought up to it.
/// A context in no group has nothing that can have rotated.
pub(super) fn pin_cut(
    store: &Store,
    projections: &Arc<RwLock<ScopeProjections>>,
    context_id: ContextId,
    position: Option<&GovernanceParentEdge>,
) -> eyre::Result<(Option<ContextGroupId>, SharedWritersResolver)> {
    let Some(group) = calimero_governance_store::get_group_for_context(store, &context_id)? else {
        return Ok((None, Arc::new(|_| Some(CellWriters::Genesis))));
    };
    let heads = match position {
        Some(edge) => edge.governance_dag_heads.clone(),
        None => ScopeProjections::namespace_current_heads(store, group).unwrap_or_default(),
    };
    ScopeProjections::refresh_for_cut(projections, store, group, &heads);
    let (store, projections) = (store.clone(), Arc::clone(projections));
    let resolver =
        memoized(move |cell| writers_at_cut(&projections, &store, &context_id, *cell, &heads));
    Ok((Some(group), resolver))
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
}

impl Publisher<'_> {
    /// Publish `rotations` signed by this node so the run's delta cites them. Nothing goes
    /// out unless all are admissible; ops published before a failing one stay published.
    pub(super) async fn publish(
        &self,
        run: RunKind,
        rotations: &[SharedRotation],
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
        if let Some(heads) = ScopeProjections::namespace_current_heads(self.store, group_id) {
            ScopeProjections::refresh_for_cut(self.projections, self.store, group_id, &heads);
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
        let (group, resolve) = pin_cut(&store(), &empty_projections(), context(), None)
            .expect("an unregistered context pins");
        assert_eq!(group, None);
        assert_eq!(resolve(&[7; 32]), Some(CellWriters::Genesis));
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
