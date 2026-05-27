use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::GetGroupMetadataRequest;
use calimero_governance_store::MetadataRepository;

use crate::{group_store, ContextManager};

impl Handler<GetGroupMetadataRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetGroupMetadataRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetGroupMetadataRequest { group_id }: GetGroupMetadataRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = MetadataRepository::new(&self.datastore).group_metadata(&group_id);
        ActorResponse::reply(result)
    }
}
