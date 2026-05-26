use crate::group_store::MetadataRepository;
use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::GetMemberMetadataRequest;

use crate::{group_store, ContextManager};

impl Handler<GetMemberMetadataRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetMemberMetadataRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetMemberMetadataRequest { group_id, member }: GetMemberMetadataRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = MetadataRepository::new(&self.datastore).member_metadata(&group_id, &member);
        ActorResponse::reply(result)
    }
}
