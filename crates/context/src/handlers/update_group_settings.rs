use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::UpdateGroupSettingsRequest;
use calimero_context_primitives::local_governance::GroupOp;

use crate::group_store;
use crate::ContextManager;

impl Handler<UpdateGroupSettingsRequest> for ContextManager {
    type Result = ActorResponse<Self, <UpdateGroupSettingsRequest as Message>::Result>;

    fn handle(
        &mut self,
        UpdateGroupSettingsRequest {
            group_id,
            requester,
            upgrade_policy,
        }: UpdateGroupSettingsRequest,
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
                    GroupOp::UpgradePolicySet {
                        policy: upgrade_policy,
                    },
                )
                .await?;

                Ok(())
            }
            .into_actor(self),
        )
    }
}
