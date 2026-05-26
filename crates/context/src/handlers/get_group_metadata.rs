#![allow(deprecated)] // #2303: per-file Repository migration deferred to follow-up

use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::GetGroupMetadataRequest;

use crate::{group_store, ContextManager};

impl Handler<GetGroupMetadataRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetGroupMetadataRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetGroupMetadataRequest { group_id }: GetGroupMetadataRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = group_store::get_group_metadata(&self.datastore, &group_id);
        ActorResponse::reply(result)
    }
}
