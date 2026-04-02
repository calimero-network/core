use actix::{ActorResponse, Handler, Message};

use calimero_context_primitives::group::BroadcastGroupLocalStateRequest;

use crate::ContextManager;

impl Handler<BroadcastGroupLocalStateRequest> for ContextManager {
    type Result = ActorResponse<Self, <BroadcastGroupLocalStateRequest as Message>::Result>;

    fn handle(
        &mut self,
        BroadcastGroupLocalStateRequest { group_id: _ }: BroadcastGroupLocalStateRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        ActorResponse::reply(Ok(()))
    }
}
