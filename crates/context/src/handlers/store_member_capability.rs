use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::StoreMemberCapabilityRequest;
use calimero_governance_store::CapabilitiesRepository;

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
        let result = CapabilitiesRepository::new(&self.datastore).set_member_capability(
            &group_id,
            &member,
            capabilities,
        );
        ActorResponse::reply(result)
    }
}
