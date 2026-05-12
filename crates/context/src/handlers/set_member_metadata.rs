use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::SetMemberMetadataRequest;
use calimero_context_client::local_governance::GroupOp;

use crate::ContextManager;

impl Handler<SetMemberMetadataRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetMemberMetadataRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetMemberMetadataRequest {
            group_id,
            member,
            name,
            data,
            requester,
        }: SetMemberMetadataRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Authorization is enforced at apply time: a member may set their own
        // metadata; otherwise the signer needs CAN_MANAGE_METADATA / admin.
        self.sign_and_publish_group_op(
            &group_id,
            requester,
            false,
            GroupOp::MemberMetadataSet { member, name, data },
        )
    }
}
