use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::StoreDefaultCapabilitiesRequest;
use calimero_governance_store::CapabilitiesRepository;

use crate::{group_store, ContextManager};

impl Handler<StoreDefaultCapabilitiesRequest> for ContextManager {
    type Result = ActorResponse<Self, <StoreDefaultCapabilitiesRequest as Message>::Result>;

    fn handle(
        &mut self,
        StoreDefaultCapabilitiesRequest {
            group_id,
            capabilities,
        }: StoreDefaultCapabilitiesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = CapabilitiesRepository::new(&self.datastore)
            .set_default_capabilities(&group_id, capabilities);
        ActorResponse::reply(result)
    }
}
