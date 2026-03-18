use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::StoreContextAliasRequest;

use crate::{group_store, ContextManager};

impl Handler<StoreContextAliasRequest> for ContextManager {
    type Result = ActorResponse<Self, <StoreContextAliasRequest as Message>::Result>;

    fn handle(
        &mut self,
        StoreContextAliasRequest {
            group_id,
            context_id,
            alias,
        }: StoreContextAliasRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result =
            group_store::set_context_alias(&self.datastore, &group_id, &context_id, &alias);
        ActorResponse::reply(result)
    }
}
