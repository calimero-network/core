use actix::{ActorResponse, Handler, Message};

use calimero_context_client::group::BroadcastGroupAliasesRequest;

use crate::{group_store, ContextManager};

impl Handler<BroadcastGroupAliasesRequest> for ContextManager {
    type Result = ActorResponse<Self, <BroadcastGroupAliasesRequest as Message>::Result>;

    fn handle(
        &mut self,
        BroadcastGroupAliasesRequest { group_id }: BroadcastGroupAliasesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let _entries = match group_store::enumerate_group_contexts_with_aliases(
            &self.datastore,
            &group_id,
            0,
            usize::MAX,
        ) {
            Ok(e) => e,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        ActorResponse::reply(Ok(()))
    }
}
