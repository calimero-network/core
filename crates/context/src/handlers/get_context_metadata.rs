use crate::group_store::MetadataRepository;
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
        let result =
            MetadataRepository::new(&self.datastore).context_metadata(&group_id, &context_id);
        ActorResponse::reply(result)
    }
}
