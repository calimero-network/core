use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::StoreGroupAliasRequest;

use crate::{group_store, ContextManager};

impl Handler<StoreGroupAliasRequest> for ContextManager {
    type Result = ActorResponse<Self, <StoreGroupAliasRequest as Message>::Result>;

    fn handle(
        &mut self,
        StoreGroupAliasRequest { group_id, alias }: StoreGroupAliasRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = group_store::set_group_alias(&self.datastore, &group_id, &alias);
        ActorResponse::reply(result)
    }
}
