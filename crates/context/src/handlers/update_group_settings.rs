use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::UpdateGroupSettingsRequest;
use calimero_context_client::local_governance::GroupOp;

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
        self.sign_and_publish_group_op(
            &group_id,
            requester,
            true,
            GroupOp::UpgradePolicySet {
                policy: upgrade_policy,
            },
        )
    }
}
