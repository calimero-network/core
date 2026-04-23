use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::messages::{ApplySignedNamespaceOpRequest, NamespaceApplyOutcome};
use calimero_dag::AddDeltaOutcome;

use crate::governance_dag::{signed_namespace_op_to_delta, NamespaceGovernanceApplier};
use crate::ContextManager;

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
                match dag.add_delta_with_outcome(delta, &applier).await {
                    Ok(AddDeltaOutcome::Applied) => Ok(NamespaceApplyOutcome::Applied),
                    Ok(AddDeltaOutcome::Pending) => Ok(NamespaceApplyOutcome::Pending),
                    Ok(AddDeltaOutcome::Duplicate) => Ok(NamespaceApplyOutcome::Duplicate),
                    Err(e) => Err(eyre::eyre!("namespace DAG apply error: {e}")),
                }
            }
            .into_actor(self),
        )
    }
}
