use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::StoreContextMetadataRequest;
use calimero_governance_store::MetadataRepository;

use crate::ContextManager;

impl Handler<StoreContextMetadataRequest> for ContextManager {
    type Result = ActorResponse<Self, <StoreContextMetadataRequest as Message>::Result>;

    fn handle(
        &mut self,
        StoreContextMetadataRequest {
            group_id,
            context_id,
            record,
        }: StoreContextMetadataRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result =
            MetadataRepository::new(&self.datastore).set_context(&group_id, &context_id, &record);
        ActorResponse::reply(result)
    }
}
