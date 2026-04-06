use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::StoreDefaultVisibilityRequest;

use crate::{group_store, ContextManager};

impl Handler<StoreDefaultVisibilityRequest> for ContextManager {
    type Result = ActorResponse<Self, <StoreDefaultVisibilityRequest as Message>::Result>;

    fn handle(
        &mut self,
        StoreDefaultVisibilityRequest { group_id, mode }: StoreDefaultVisibilityRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = group_store::set_default_visibility(&self.datastore, &group_id, mode);
        ActorResponse::reply(result)
    }
}
