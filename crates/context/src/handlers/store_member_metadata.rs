#![allow(deprecated)] // #2303: per-file Repository migration deferred to follow-up

use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::StoreMemberMetadataRequest;

use crate::{group_store, ContextManager};

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
        let result = group_store::set_member_metadata(&self.datastore, &group_id, &member, &record);
        ActorResponse::reply(result)
    }
}
