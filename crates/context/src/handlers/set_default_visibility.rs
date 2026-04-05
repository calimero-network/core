use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::SetDefaultVisibilityRequest;
use calimero_context_client::local_governance::GroupOp;
use tracing::info;

use crate::group_store;
use crate::ContextManager;

impl Handler<SetDefaultVisibilityRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetDefaultVisibilityRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetDefaultVisibilityRequest {
            group_id,
            default_visibility,
            requester,
        }: SetDefaultVisibilityRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let preflight = match self.governance_preflight(&group_id, requester, true) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let mode_u8 = match default_visibility {
            calimero_context_config::VisibilityMode::Open => 0u8,
            calimero_context_config::VisibilityMode::Restricted => 1u8,
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
                    GroupOp::DefaultVisibilitySet { mode: mode_u8 },
                )
                .await?;

                info!(
                    ?group_id,
                    ?default_visibility,
                    "default context visibility updated"
                );

                Ok(())
            }
            .into_actor(self),
        )
    }
}
