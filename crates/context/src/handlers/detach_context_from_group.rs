use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::DetachContextFromGroupRequest;
use calimero_context_client::local_governance::GroupOp;
use eyre::bail;

use crate::governance_broadcast::observe_handler_delivery;
use crate::group_store;
use crate::ContextManager;

impl Handler<DetachContextFromGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <DetachContextFromGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        DetachContextFromGroupRequest {
            group_id,
            context_id,
            requester,
        }: DetachContextFromGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let preflight = match self.governance_preflight(&group_id, requester, true) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        if let Err(err) = (|| -> eyre::Result<()> {
            let current_group = group_store::get_group_for_context(&self.datastore, &context_id)?;
            if current_group.as_ref() != Some(&group_id) {
                bail!("context '{context_id}' does not belong to group '{group_id:?}'");
            }
            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        let datastore = preflight.datastore.clone();
        let node_client = preflight.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);
        let sk = preflight.signer_sk();

        ActorResponse::r#async(
            async move {
                let report = group_store::sign_apply_and_publish(
                    &datastore,
                    &node_client,
                    &ack_router,
                    &group_id,
                    &sk,
                    GroupOp::ContextDetached { context_id },
                )
                .await?;
                if let Some(report) = report.as_ref() {
                    observe_handler_delivery(
                        "detach_context_from_group",
                        "ContextDetached",
                        report,
                    );
                }

                Ok(())
            }
            .into_actor(self),
        )
    }
}
