use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::GetGroupUpgradeStatusRequest;

use crate::{group_store, ContextManager};

impl Handler<GetGroupUpgradeStatusRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetGroupUpgradeStatusRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetGroupUpgradeStatusRequest { group_id }: GetGroupUpgradeStatusRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = group_store::load_group_upgrade(&self.datastore, &group_id);
        ActorResponse::reply(result)
    }
}
