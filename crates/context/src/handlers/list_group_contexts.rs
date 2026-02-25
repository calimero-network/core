use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::ListGroupContextsRequest;

use crate::{group_store, ContextManager};

impl Handler<ListGroupContextsRequest> for ContextManager {
    type Result = ActorResponse<Self, <ListGroupContextsRequest as Message>::Result>;

    fn handle(
        &mut self,
        ListGroupContextsRequest {
            group_id,
            offset,
            limit,
        }: ListGroupContextsRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result =
            group_store::enumerate_group_contexts(&self.datastore, &group_id, offset, limit);

        ActorResponse::reply(result)
    }
}
