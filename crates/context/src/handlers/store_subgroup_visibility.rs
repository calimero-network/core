use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::StoreSubgroupVisibilityRequest;
use calimero_context_config::VisibilityMode;

use crate::{group_store, ContextManager};

impl Handler<StoreSubgroupVisibilityRequest> for ContextManager {
    type Result = ActorResponse<Self, <StoreSubgroupVisibilityRequest as Message>::Result>;

    fn handle(
        &mut self,
        StoreSubgroupVisibilityRequest { group_id, mode }: StoreSubgroupVisibilityRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let visibility = match mode {
            0 => VisibilityMode::Open,
            _ => VisibilityMode::Restricted,
        };
        let result = group_store::set_subgroup_visibility(&self.datastore, &group_id, visibility);
        ActorResponse::reply(result)
    }
}
