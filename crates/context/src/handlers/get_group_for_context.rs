use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::GetGroupForContextRequest;

use crate::group_store;
use crate::ContextManager;

impl Handler<GetGroupForContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetGroupForContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetGroupForContextRequest { context_id }: GetGroupForContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = group_store::get_group_for_context(&self.datastore, &context_id);

        ActorResponse::reply(result)
    }
}
