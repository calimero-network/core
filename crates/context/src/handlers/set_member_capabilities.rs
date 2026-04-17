use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::SetMemberCapabilitiesRequest;
use calimero_context_client::local_governance::GroupOp;

use crate::{group_store, ContextManager};

impl Handler<SetMemberCapabilitiesRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetMemberCapabilitiesRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetMemberCapabilitiesRequest {
            group_id,
            member,
            capabilities,
            requester,
        }: SetMemberCapabilitiesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Verify the target member actually exists in the group
        if group_store::get_group_member_role(&self.datastore, &group_id, &member)
            .ok()
            .flatten()
            .is_none()
        {
            return ActorResponse::reply(Err(eyre::eyre!(
                "identity is not a member of group '{group_id:?}'"
            )));
        }

        self.sign_and_publish_group_op(
            &group_id,
            requester,
            true,
            GroupOp::MemberCapabilitySet {
                member,
                capabilities,
            },
        )
    }
}
