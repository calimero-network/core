use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::SetDefaultCapabilitiesRequest;
use calimero_context_client::local_governance::GroupOp;

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
        self.sign_and_publish_group_op(
            &group_id,
            requester,
            true,
            GroupOp::DefaultCapabilitiesSet {
                capabilities: default_capabilities,
            },
        )
    }
}
