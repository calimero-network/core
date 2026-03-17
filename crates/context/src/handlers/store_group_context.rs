use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::StoreGroupContextRequest;

use crate::{group_store, ContextManager};

impl Handler<StoreGroupContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <StoreGroupContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        StoreGroupContextRequest { group_id, context_id }: StoreGroupContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result =
            group_store::register_context_in_group(&self.datastore, &group_id, &context_id);
        ActorResponse::reply(result)
    }
}
