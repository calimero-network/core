use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::SetContextMetadataRequest;
use calimero_context_client::local_governance::GroupOp;

use crate::ContextManager;

impl Handler<SetContextMetadataRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetContextMetadataRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetContextMetadataRequest {
            group_id,
            context_id,
            name,
            data,
            requester,
        }: SetContextMetadataRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        self.sign_and_publish_group_op(
            &group_id,
            requester,
            false,
            GroupOp::ContextMetadataSet {
                context_id,
                name,
                data,
            },
        )
    }
}
