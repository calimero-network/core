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

        let delta = match signed_namespace_op_to_delta(&op) {
            Ok(d) => d,
            Err(e) => return ActorResponse::reply(Err(e)),
        };

        let applier = NamespaceGovernanceApplier::new(datastore);

        ActorResponse::r#async(
            async move {
                let mut dag = dag.lock().await;
                let outcome = dag.add_delta_with_outcome(delta, &applier).await;

                // Bound this namespace's in-memory governance-DAG history. A hot
                // namespace that never gets evicted from `namespace_dags` would
                // otherwise retain every applied op for the process lifetime.
                // Done under the held DAG lock (the apply path is the only
                // writer), and only after `Applied` advanced the frontier.
                //
                // Lossless for peers: applied ops are durably persisted and the
                // backfill responder serves from RocksDB, not this DAG — so the
                // pruned ids are discarded, NOT deleted from disk.
                if matches!(outcome, Ok(AddDeltaOutcome::Applied))
                    && dag.delta_count() > NAMESPACE_DAG_PRUNE_THRESHOLD
                {
                    let pruned = dag.prune_to_recent(NAMESPACE_DAG_PRUNE_RETAIN);
                    if !pruned.is_empty() {
                        tracing::debug!(
                            namespace = ?namespace_id,
                            pruned = pruned.len(),
                            retained = dag.delta_count(),
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
