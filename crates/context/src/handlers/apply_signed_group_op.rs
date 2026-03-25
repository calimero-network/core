use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_config::types::ContextGroupId;
use calimero_context_primitives::messages::ApplySignedGroupOpRequest;

use crate::governance_dag::{signed_op_to_delta, GroupGovernanceApplier};
use crate::ContextManager;

impl Handler<ApplySignedGroupOpRequest> for ContextManager {
    type Result = ActorResponse<Self, <ApplySignedGroupOpRequest as Message>::Result>;

    fn handle(
        &mut self,
        ApplySignedGroupOpRequest { op }: ApplySignedGroupOpRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let group_id = ContextGroupId::from(op.group_id);
        let dag = self.get_or_create_group_dag(&group_id);
        let datastore = self.datastore.clone();

        let delta = match signed_op_to_delta(&op) {
            Ok(d) => d,
            Err(e) => return ActorResponse::reply(Err(e)),
        };

        let applier = GroupGovernanceApplier::new(datastore);

        ActorResponse::r#async(
            async move {
                let mut dag = dag.lock().await;
                match dag.add_delta(delta, &applier).await {
                    Ok(true) => Ok(()),
                    Ok(false) => {
                        tracing::debug!("group op queued as pending (waiting for parents)");
                        Ok(())
                    }
                    Err(e) => Err(eyre::eyre!("DAG apply error: {e}")),
                }
            }
            .into_actor(self),
        )
    }
}
