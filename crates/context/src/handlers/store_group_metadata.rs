use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::StoreGroupMetadataRequest;

use crate::{group_store, ContextManager};

impl Handler<StoreGroupMetadataRequest> for ContextManager {
    type Result = ActorResponse<Self, <StoreGroupMetadataRequest as Message>::Result>;

    fn handle(
        &mut self,
        StoreGroupMetadataRequest { group_id, record }: StoreGroupMetadataRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = group_store::set_group_metadata(&self.datastore, &group_id, &record);
        ActorResponse::reply(result)
    }
}
