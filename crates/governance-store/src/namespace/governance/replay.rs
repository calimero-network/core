//! The key-arrival replay: every op a key opens, root and group, judged in one causal
//! order, so no verdict rests on an ancestor of the other kind not yet replayed.

use std::collections::{BTreeSet, HashSet};

use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
use eyre::Result as EyreResult;

use super::super::op_log::NamespaceOpLogService;
use super::super::retry::{order_causally, NamespaceRetryService, ReplayCandidate, RetryCandidate};
use super::{ensure_subgroup_sealed_carries_a_join, open_relayed_join, NamespaceGovernance};
use crate::authorizer::AtCutAuthorizer;
use crate::held_ops::HeldOps;
use crate::metrics::record_namespace_retry_event;
use crate::void_ledger::Parked;
use crate::{load_nonce_window, DivergenceReport, GroupKeyring};

/// What a key-arrival replay achieved.
#[derive(Default)]
pub(super) struct Replay {
    /// Ops that landed: a parked root op, or a group op whose nonce this replay spent.
    pub(super) applied: usize,
    /// The last post-apply divergence a replayed op reported; no later arrival of the
    /// same op checks it again.
    pub(super) divergence: Option<DivergenceReport>,
}

/// What replaying one op did.
#[derive(Default)]
struct Step {
    settled: bool,           // judged for good, or nothing a later round could change
    applied: bool,           // landed this time
    opens: Option<[u8; 32]>, // a group whose ops the op created or keyed
}

impl Step {
    fn settled() -> Self {
        Self {
            settled: true,
            ..Self::default()
        }
    }
}

impl NamespaceGovernance<'_> {
    /// Replay every op this node can open, root and group, in one causal order, in rounds
    /// while one finds an op not seen before (a key or group a landed op opens) or lands one.
    pub(super) fn replay(&self, target: [u8; 32], depth: u8) -> EyreResult<Replay> {
        if depth > 0 {
            tracing::debug!(
                namespace_id = %hex::encode(self.namespace_id.as_bytes()),
                depth,
                "an op replayed by a key arrival joins that replay instead of starting one"
            );
            return Ok(Replay::default());
        }
        let mut replay = Replay::default();
        let outcome = self.replay_rounds(target, depth, &mut replay);
        // A key that arrives late replays ops that may include a removal, or ops a
        // removal voids; what they wrote is rebuilt once, even if a later round failed.
        let rebuilt = if replay.applied > 0 {
            self.reconcile_voided(None)
        } else {
            Ok(())
        };
        outcome.and(rebuilt).map(|()| replay)
    }

    fn replay_rounds(&self, target: [u8; 32], depth: u8, replay: &mut Replay) -> EyreResult<()> {
        let ns = self.namespace_id.to_bytes();
        let retry = NamespaceRetryService::new(self.store, self.namespace_id);
        let op_log = NamespaceOpLogService::new(self.store, self.namespace_id);

        let mut opened = BTreeSet::from([target]);
        let mut seen: HashSet<[u8; 32]> = HashSet::new();
        let mut settled: HashSet<[u8; 32]> = HashSet::new();
        let mut landed_last_round = false;
        loop {
            // What was folded before the key arrived, or a round ago, is stale now.
            self.authorizer.forget();
            let mut groups = opened.clone();
            groups.extend(
                HeldOps::new(self.store, self.namespace_id)
                    .read()?
                    .ops
                    .iter()
                    .map(|held| held.group_id),
            );
            if target == ns {
                groups.extend(retry.groups_with_held_key_buffered_ops()?);
            }
            let mut candidates = Vec::new();
            let mut fresh = false;
            for candidate in retry.collect_replay_candidates(&groups)? {
                let id = candidate.id()?;
                if !settled.contains(&id) {
                    fresh |= seen.insert(id);
                    candidates.push(candidate);
                }
            }
            if !fresh && !(landed_last_round && !candidates.is_empty()) {
                break;
            }
            landed_last_round = false;
            record_namespace_retry_event("collected");
            for candidate in order_causally(&op_log, candidates)? {
                let id = candidate.id()?;
                let step = match &candidate {
                    ReplayCandidate::Root { signed_op, key } => {
                        self.replay_root(signed_op, key, depth, replay)?
                    }
                    ReplayCandidate::Group(candidate) => self.replay_group(candidate, replay)?,
                };
                if step.settled {
                    let _ = settled.insert(id);
                }
                if step.applied {
                    replay.applied += 1;
                    landed_last_round = true;
                }
                opened.extend(step.opens);
            }
        }
        if seen.is_empty() {
            record_namespace_retry_event("none");
        }
        Ok(())
    }

    /// Replay a sealed root op `key` opens, running what its arrival would have run.
    fn replay_root(
        &self,
        parked: &SignedNamespaceOp,
        key: &[u8; 32],
        depth: u8,
        replay: &mut Replay,
    ) -> EyreResult<Step> {
        // This node signed it, so it held the key and applied it then.
        if self.authored_here(parked)? {
            self.settle_parked(parked, true);
            return Ok(Step::settled());
        }
        let state = self.parked_state(parked);
        if state == Some(Parked::Refused) {
            return Ok(Step::settled());
        }
        let Some((gate_op, root)) = self.open_sealed_root(parked, key) else {
            self.settle_parked(parked, false);
            return Ok(Step::settled());
        };
        // The check the receive path runs after unsealing.
        if let Err(e) = root.validate_after_unsealing() {
            tracing::warn!(
                namespace_id = %hex::encode(self.namespace_id.as_bytes()),
                error = %format!("{e:#}"),
                "skipping a sealed root op that fails validation after unsealing"
            );
            self.settle_parked(parked, false);
            return Ok(Step::settled());
        }
        // A create folded long ago is not replay-safe: its fold seats the creator as
        // admin again. A parked create was never applied here, so it is judged.
        if state.is_none() && self.group_created_already_folded(&root)? {
            return Ok(Step::settled());
        }
        if self.root_op_is_void(&gate_op, &root, gate_op.content_hash()?)? {
            self.settle_parked(parked, true);
            return Ok(Step::settled());
        }
        let outcome = match state {
            Some(state) => self.replay_parked_root_op(parked, &gate_op, &root, state),
            None => self.apply_root_op(&gate_op, &root),
        };
        if let Err(e) = outcome {
            tracing::warn!(
                namespace_id = %hex::encode(self.namespace_id.as_bytes()),
                error = %format!("{e:#}"),
                "sealed root op failed to apply on retry"
            );
            return Ok(Step::default());
        }
        record_namespace_retry_event("sealed_root_applied");
        // The rest of what an arriving op does, best-effort: the fold already happened.
        match self.root_op_side_effects(&gate_op, &root, depth + 1) {
            Ok(effects) => {
                if effects.divergence.is_some() {
                    replay.divergence = effects.divergence;
                }
                for miss in effects.key_unwrap_failures {
                    tracing::debug!(
                        namespace_id = %hex::encode(self.namespace_id.as_bytes()),
                        group_id = %hex::encode(miss.group_id),
                        reason = %miss.reason,
                        "a replayed root op's effect still lacks a key"
                    );
                }
            }
            Err(e) => tracing::warn!(
                namespace_id = %hex::encode(self.namespace_id.as_bytes()),
                error = %format!("{e:#}"),
                "a replayed root op folded but one of its effects did not"
            ),
        }
        // An op applied on arrival carries no mark; folding it again changed nothing.
        Ok(Step {
            settled: true,
            applied: state.is_some(),
            opens: state.and_then(|_| group_opened_by(&root)),
        })
    }

    /// Open a sealed root op as its gates must see it: the envelope, or a relayed join's
    /// inner op. `None` when it does not open, or a group-sealed one carries no join.
    fn open_sealed_root(
        &self,
        parked: &SignedNamespaceOp,
        key: &[u8; 32],
    ) -> Option<(SignedNamespaceOp, RootOp)> {
        let opened = match &parked.op {
            NamespaceOp::RootSealed { encrypted, .. } => {
                GroupKeyring::decrypt_root_op(key, encrypted).map(|root| (parked.clone(), root))
            }
            NamespaceOp::RootSealedForGroup {
                group_id,
                encrypted,
                ..
            } => GroupKeyring::decrypt_root_op(key, encrypted).and_then(|root| {
                ensure_subgroup_sealed_carries_a_join(*group_id, self.namespace_id, &root)
                    .map(|()| (parked.clone(), root))
            }),
            NamespaceOp::RootRelaySealed { encrypted, .. } => {
                open_relayed_join(self.namespace_id, key, encrypted).and_then(|inner| match &inner
                    .op
                {
                    NamespaceOp::Root(root) => {
                        let root = root.clone();
                        Ok((inner, root))
                    }
                    _ => Err(eyre::eyre!("relayed op is not a root op after unsealing")),
                })
            }
            _ => return None,
        };
        opened
            .map_err(|e| {
                tracing::warn!(
                    namespace_id = %hex::encode(self.namespace_id.as_bytes()),
                    error = %format!("{e:#}"),
                    "skipping a sealed root op that would not open on retry"
                );
            })
            .ok()
    }

    /// Replay a readable group op.
    fn replay_group(&self, candidate: &RetryCandidate, replay: &mut Replay) -> EyreResult<Step> {
        let NamespaceOp::Group {
            group_id,
            ref encrypted,
            ..
        } = candidate.signed_op.op
        else {
            return Ok(Step::settled());
        };
        let parked = self.parked_state(&candidate.signed_op);
        if parked == Some(Parked::Refused) {
            return Ok(Step::settled());
        }
        let delta_id = candidate.signed_op.content_hash()?;
        let held_ops = HeldOps::new(self.store, self.namespace_id);
        // A held op waits on its group's own key only while that group's history is what
        // it lacks; one held for another hole is judged once its cut reads whole.
        let sealed_in_group = |judge: &dyn AtCutAuthorizer| {
            judge.history_sealed_in_group(&group_id, &candidate.signed_op.parent_op_hashes)
        };
        if GroupKeyring::new(self.store, group_id)
            .load_current_key()?
            .is_none()
            && held_ops.contains(delta_id)?
            && self.authorizer.at_cut().is_none_or(sealed_in_group)
        {
            record_namespace_retry_event("still_held");
            return Ok(Step::default());
        }
        let signer = &candidate.signed_op.signer;
        let nonce = candidate.signed_op.nonce;
        // An already-windowed nonce replays as `Ok(None)` and writes nothing.
        let windowed = || {
            load_nonce_window(self.store, &group_id, signer)
                .map(|window| window.contains(nonce))
                .unwrap_or(false)
        };
        let was_windowed = windowed();
        match self.replay_group_op(
            &candidate.signed_op,
            &group_id,
            &candidate.group_key,
            encrypted,
            parked,
        ) {
            Ok(divergence) => {
                let rotated = self.replay_deferred_rotation(candidate, parked.is_some());
                let applied = !was_windowed && windowed();
                if applied {
                    record_namespace_retry_event("applied");
                    held_ops.release(delta_id)?;
                    tracing::info!(
                        group_id = %hex::encode(group_id.to_bytes()),
                        "retried encrypted op after KeyDelivery"
                    );
                } else {
                    record_namespace_retry_event("nonce_skip");
                }
                if divergence.is_some() {
                    replay.divergence = divergence;
                }
                Ok(Step {
                    settled: true,
                    applied,
                    opens: rotated.then_some(group_id.to_bytes()),
                })
            }
            Err(e) if self.still_held(candidate, &e) => {
                record_namespace_retry_event("still_held");
                Ok(Step::default())
            }
            Err(e) => {
                record_namespace_retry_event("failed");
                tracing::warn!(
                    group_id = %hex::encode(group_id.to_bytes()),
                    error = %format!("{e:#}"),
                    "failed to replay a group op once its key arrived"
                );
                Ok(Step::default())
            }
        }
    }
}

/// The group whose ops `root` may make readable or applicable: one it created or
/// delivered a key for.
fn group_opened_by(root: &RootOp) -> Option<[u8; 32]> {
    match root {
        RootOp::OnBehalf { op, .. } | RootOp::RootGuarded { op, .. } => group_opened_by(op),
        RootOp::KeyDelivery { group_id, .. } | RootOp::GroupCreated { group_id, .. } => {
            Some(group_id.to_bytes())
        }
        _ => None,
    }
}
