use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::messages::ApplySignedNamespaceOpRequest;

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
                match dag.add_delta(delta, &applier).await {
                    Ok(_applied) => Ok(()),
                    Err(e) => Err(eyre::eyre!("namespace DAG apply error: {e}")),
                }
            }
            .into_actor(self),
        )
    }
}
