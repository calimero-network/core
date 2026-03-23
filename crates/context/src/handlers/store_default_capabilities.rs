use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::StoreDefaultCapabilitiesRequest;

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
        let result =
            group_store::set_default_capabilities(&self.datastore, &group_id, capabilities);
        ActorResponse::reply(result)
    }
}
