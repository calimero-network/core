use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::StoreMemberAliasRequest;

use crate::{group_store, ContextManager};

impl Handler<StoreMemberAliasRequest> for ContextManager {
    type Result = ActorResponse<Self, <StoreMemberAliasRequest as Message>::Result>;

    fn handle(
        &mut self,
        StoreMemberAliasRequest {
            group_id,
            member,
            alias,
        }: StoreMemberAliasRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = group_store::set_member_alias(&self.datastore, &group_id, &member, &alias);
        ActorResponse::reply(result)
    }
}
