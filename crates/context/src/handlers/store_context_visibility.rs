use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::StoreContextVisibilityRequest;

use crate::{group_store, ContextManager};

impl Handler<StoreContextVisibilityRequest> for ContextManager {
    type Result = ActorResponse<Self, <StoreContextVisibilityRequest as Message>::Result>;

    fn handle(
        &mut self,
        StoreContextVisibilityRequest {
            group_id,
            context_id,
            mode,
            creator,
        }: StoreContextVisibilityRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = group_store::set_context_visibility(
            &self.datastore,
            &group_id,
            &context_id,
            mode,
            *creator,
        );
        ActorResponse::reply(result)
    }
}
