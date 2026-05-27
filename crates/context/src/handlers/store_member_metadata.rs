use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::StoreMemberMetadataRequest;
use calimero_governance_store::MetadataRepository;

use crate::ContextManager;

impl Handler<StoreMemberMetadataRequest> for ContextManager {
    type Result = ActorResponse<Self, <StoreMemberMetadataRequest as Message>::Result>;

    fn handle(
        &mut self,
        StoreMemberMetadataRequest {
            group_id,
            member,
            record,
        }: StoreMemberMetadataRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result =
            MetadataRepository::new(&self.datastore).set_member(&group_id, &member, &record);
        ActorResponse::reply(result)
    }
}
