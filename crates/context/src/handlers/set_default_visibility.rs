use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::SetDefaultVisibilityRequest;
use calimero_context_client::local_governance::GroupOp;

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
        let mode_u8 = match default_visibility {
            calimero_context_config::VisibilityMode::Open => 0u8,
            calimero_context_config::VisibilityMode::Restricted => 1u8,
        };

        self.sign_and_publish_group_op(
            &group_id,
            requester,
            true,
            GroupOp::DefaultVisibilitySet { mode: mode_u8 },
        )
    }
}
