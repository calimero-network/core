use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::messages::{ApplySignedNamespaceOpRequest, NamespaceApplyOutcome};
use calimero_dag::AddDeltaOutcome;

use crate::governance_dag::{signed_namespace_op_to_delta, NamespaceGovernanceApplier};
use crate::{ContextManager, NAMESPACE_DAG_PRUNE_RETAIN, NAMESPACE_DAG_PRUNE_THRESHOLD};

impl Handler<ApplySignedNamespaceOpRequest> for ContextManager {
    type Result = ActorResponse<Self, <ApplySignedNamespaceOpRequest as Message>::Result>;

    fn handle(
        &mut self,
        ApplySignedNamespaceOpRequest { op }: ApplySignedNamespaceOpRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let namespace_id = op.namespace_id;
        let dag = self.get_or_create_namespace_dag(&namespace_id);
        let datastore = self.datastore.clone();
        // Separate handle for the shadow-compare (the one above is moved into
        // the applier).
        let compare_store = self.datastore.clone();

        let delta = match signed_namespace_op_to_delta(&op) {
            Ok(d) => d,
            Err(e) => return ActorResponse::reply(Err(e)),
        };

        let applier = NamespaceGovernanceApplier::new(datastore);

        // Shadow unified-op projection (additive — nothing reads it yet): derive
        // the op this governance delta represents, to fold into its scope's
        // projection only if the DAG actually applies it. Built before the DAG
        // call consumes `delta`; `None` for ops not yet in the projection model.
        let shadow_op = crate::scope_projection::op_from_signed_namespace_op(
            &delta.payload,
            delta.hlc,
            &delta.parents,
        );
        let scope_projections = Arc::clone(&self.scope_projections);

        ActorResponse::r#async(
            async move {
                let mut dag = dag.lock().await;
                let outcome = dag.add_delta_with_outcome(delta, &applier).await;

                // Fold into the shadow projection only on a real apply, mirroring
                // the divergence-outbox gating below. A poisoned lock is ignored:
                // the projection is not yet authoritative, so it must never break
                // the governance apply path.
                if matches!(outcome, Ok(AddDeltaOutcome::Applied)) {
                    if let Some(op) = &shadow_op {
                        if let Ok(mut projections) = scope_projections.lock() {
                            projections.ingest_op(op);
                        }

                        // Shadow-compare (additive, log-only): the membership op
                        // we just fed must leave the projection agreeing with the
                        // live resolver for the member it touched. Per-member (not
                        // full-set) so a partially-fed projection — e.g. right
                        // after restart — can't false-positive. Conservative gate:
                        // only flag when the live resolver has a DIRECT row the
                        // projection disagrees with; skip inherited / open-join
                        // members the live system doesn't store directly but the
                        // projection models as direct (an expected model
                        // difference, not a feed bug). This is the projection's
                        // first reader — the precursor to authorizing against it.
                        let membership = match &op.payload {
                            calimero_op::OpPayload::MemberAdded { group, member, .. }
                            | calimero_op::OpPayload::MemberRemoved { group, member } => {
                                Some((*group, *member))
                            }
                            _ => None,
                        };
                        if let Some((group, member)) = membership {
                            let projected = scope_projections
                                .lock()
                                .ok()
                                .and_then(|p| p.role_of(&op.scope, &group, &member));
                            let live = calimero_governance_store::MembershipRepository::new(
                                &compare_store,
                            )
                            .role_of(&group, &member)
                            .ok()
                            .flatten();
                            if live.is_some() && projected != live {
                                tracing::warn!(
                                    marker = "unified_projection_divergence",
                                    plane = "membership",
                                    ?group,
                                    %member,
                                    ?projected,
                                    ?live,
                                    "unified-op projection disagrees with live membership resolver"
                                );
                            }
                        }
                    }
                }

                // Bound this namespace's in-memory governance-DAG history. A hot
                // namespace that never gets evicted from `namespace_dags` would
                // otherwise retain every applied op for the process lifetime.
                // Done under the held DAG lock (the apply path is the only
                // writer), and only after `Applied` advanced the frontier.
                //
                // Gate on the *applied* count, not the total `delta_count()`
                // (which also counts pending deltas). `prune_to_recent` only
                // prunes applied, non-head history, so triggering off a large
                // *pending* backlog (e.g. a partition with missing parents)
                // would re-walk the whole DAG on every apply while pruning
                // nothing. Applied history advancing past the threshold is the
                // only thing this prune can actually act on; once it fires it
                // drops applied back to the retain window, so the next prune is
                // ~RETAIN..THRESHOLD applies away (a built-in hysteresis band).
                //
                // Lossless for peers: applied ops are durably persisted and the
                // backfill responder serves from RocksDB, not this DAG — so the
                // pruned ids are discarded, NOT deleted from disk.
                if matches!(outcome, Ok(AddDeltaOutcome::Applied))
                    && dag.stats().applied_deltas > NAMESPACE_DAG_PRUNE_THRESHOLD
                {
                    let pruned = dag.prune_to_recent(NAMESPACE_DAG_PRUNE_RETAIN);
                    if !pruned.is_empty() {
                        tracing::debug!(
                            namespace = ?namespace_id,
                            pruned = pruned.len(),
                            retained = dag.stats().applied_deltas,
                            "pruned applied governance-DAG history (durable op-log retained)"
                        );
                    }
                }

                // Read-and-clear the applier's divergence outbox after
                // the DAG call returns. The outbox is populated by the
                // applier's `apply` impl when `MemberRemoved` /
                // `MemberLeft` verify reports a state-hash mismatch.
                // Only meaningful on `Applied` — `Pending` / `Duplicate`
                // don't run the apply path.
                let divergence = applier.take_divergence();
                match outcome {
                    Ok(AddDeltaOutcome::Applied) => {
                        Ok(NamespaceApplyOutcome::Applied { divergence })
                    }
                    Ok(AddDeltaOutcome::Pending) => Ok(NamespaceApplyOutcome::Pending),
                    Ok(AddDeltaOutcome::Duplicate) => Ok(NamespaceApplyOutcome::Duplicate),
                    Err(e) => Err(eyre::eyre!("namespace DAG apply error: {e}")),
                }
            }
            .into_actor(self),
        )
    }
}
