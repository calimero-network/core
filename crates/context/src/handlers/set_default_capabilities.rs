use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::SetDefaultCapabilitiesRequest;
use calimero_context_client::local_governance::GroupOp;
use tracing::info;

use crate::group_store;
use crate::ContextManager;

impl Handler<SetDefaultCapabilitiesRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetDefaultCapabilitiesRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetDefaultCapabilitiesRequest {
            group_id,
            default_capabilities,
            requester,
        }: SetDefaultCapabilitiesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let preflight = match self.governance_preflight(&group_id, requester, true) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let datastore = preflight.datastore.clone();
        let node_client = preflight.node_client.clone();
        let sk = preflight.signer_sk();

        ActorResponse::r#async(
            async move {
                group_store::sign_apply_and_publish(
                    &datastore,
                    &node_client,
                    &group_id,
                    &sk,
                    GroupOp::DefaultCapabilitiesSet {
                        capabilities: default_capabilities,
                    },
                )
                .await?;

                info!(
                    ?group_id,
                    default_capabilities, "default member capabilities updated"
                );

                Ok(())
            }
            .into_actor(self),
        )
    }
}
