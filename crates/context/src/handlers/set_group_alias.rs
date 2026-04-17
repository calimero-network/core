use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::SetGroupAliasRequest;
use calimero_context_client::local_governance::GroupOp;

use crate::ContextManager;

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
        self.sign_and_publish_group_op(&group_id, requester, true, GroupOp::GroupAliasSet { alias })
    }
}
