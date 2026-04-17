use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::SetMemberAliasRequest;
use calimero_context_client::local_governance::GroupOp;

use crate::ContextManager;

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
        // Members may only set their own alias — check before preflight
        let preflight = match self.governance_preflight(&group_id, requester, false) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        if preflight.requester != member {
            return ActorResponse::reply(Err(eyre::eyre!("members may only set their own alias")));
        }

        self.sign_and_publish_group_op(
            &group_id,
            Some(preflight.requester),
            false,
            GroupOp::MemberAliasSet { member, alias },
        )
    }
}
