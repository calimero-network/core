use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::SetMemberAliasRequest;
use calimero_context_client::local_governance::GroupOp;
use tracing::info;

use crate::{group_store, ContextManager};

impl Handler<SetMemberAliasRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetMemberAliasRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetMemberAliasRequest {
            group_id,
            member,
            alias,
            requester,
        }: SetMemberAliasRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let preflight = match self.governance_preflight(&group_id, requester, false) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        if preflight.requester != member {
            return ActorResponse::reply(Err(eyre::eyre!("members may only set their own alias")));
        }

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
                    GroupOp::MemberAliasSet {
                        member,
                        alias: alias.clone(),
                    },
                )
                .await?;

                info!(
                    ?group_id,
                    %member,
                    %alias_for_log,
                    "group member alias set"
                );

                Ok(())
            }
            .into_actor(self),
        )
    }
}
