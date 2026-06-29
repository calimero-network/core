use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::StoreSubgroupVisibilityRequest;
use calimero_governance_store::CapabilitiesRepository;

use crate::ContextManager;

impl Handler<StoreSubgroupVisibilityRequest> for ContextManager {
    type Result = ActorResponse<Self, <StoreSubgroupVisibilityRequest as Message>::Result>;

    fn handle(
        &mut self,
        StoreSubgroupVisibilityRequest { group_id, mode }: StoreSubgroupVisibilityRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result =
            CapabilitiesRepository::new(&self.datastore).set_subgroup_visibility(&group_id, mode);
        ActorResponse::reply(result)
    }
}
