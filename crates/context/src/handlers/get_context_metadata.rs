#![allow(deprecated)] // #2303: per-file Repository migration deferred to follow-up

use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::GetContextMetadataRequest;

use crate::{group_store, ContextManager};

impl Handler<GetContextMetadataRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetContextMetadataRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetContextMetadataRequest {
            group_id,
            context_id,
        }: GetContextMetadataRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = group_store::get_context_metadata(&self.datastore, &group_id, &context_id);
        ActorResponse::reply(result)
    }
}
