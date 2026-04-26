use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::SetSubgroupVisibilityRequest;
use calimero_context_client::local_governance::GroupOp;

use crate::ContextManager;

impl Handler<SetSubgroupVisibilityRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetSubgroupVisibilityRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetSubgroupVisibilityRequest {
            group_id,
            subgroup_visibility,
            requester,
        }: SetSubgroupVisibilityRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let mode_u8 = match subgroup_visibility {
            calimero_context_config::VisibilityMode::Open => 0u8,
            calimero_context_config::VisibilityMode::Restricted => 1u8,
        };

        self.sign_and_publish_group_op(
            &group_id,
            requester,
            true,
            GroupOp::SubgroupVisibilitySet { mode: mode_u8 },
        )
    }
}
