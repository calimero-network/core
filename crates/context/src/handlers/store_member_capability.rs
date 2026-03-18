use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::StoreMemberCapabilityRequest;

use crate::{group_store, ContextManager};

impl Handler<StoreMemberCapabilityRequest> for ContextManager {
    type Result = ActorResponse<Self, <StoreMemberCapabilityRequest as Message>::Result>;

    fn handle(
        &mut self,
        StoreMemberCapabilityRequest {
            group_id,
            member,
            capabilities,
        }: StoreMemberCapabilityRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result =
            group_store::set_member_capability(&self.datastore, &group_id, &member, capabilities);
        ActorResponse::reply(result)
    }
}
