use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::SetGroupMetadataRequest;
use calimero_context_client::local_governance::GroupOp;

use crate::ContextManager;

impl Handler<SetGroupMetadataRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetGroupMetadataRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetGroupMetadataRequest {
            group_id,
            name,
            data,
            requester,
        }: SetGroupMetadataRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        self.sign_and_publish_group_op(
            &group_id,
            requester,
            false,
            GroupOp::GroupMetadataSet { name, data },
        )
    }
}
