use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::SetGroupAliasRequest;
use calimero_context_primitives::local_governance::GroupOp;
use tracing::info;

use crate::{group_store, ContextManager};

impl Handler<SetGroupAliasRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetGroupAliasRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetGroupAliasRequest {
            group_id,
            alias,
            requester,
        }: SetGroupAliasRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let preflight = match self.governance_preflight(&group_id, requester, true) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let datastore = preflight.datastore.clone();
        let node_client = preflight.node_client.clone();
        let sk = preflight.signer_sk();
        let alias_for_log = alias.clone();

        ActorResponse::r#async(
            async move {
                group_store::sign_apply_and_publish(
                    &datastore,
                    &node_client,
                    &group_id,
                    &sk,
                    GroupOp::GroupAliasSet {
                        alias: alias.clone(),
                    },
                )
                .await?;

                info!(?group_id, %alias_for_log, "group alias set");

                Ok(())
            }
            .into_actor(self),
        )
    }
}
