use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::GetGroupUpgradeStatusRequest;
use eyre::bail;

use crate::{group_store, ContextManager};

impl Handler<GetGroupUpgradeStatusRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetGroupUpgradeStatusRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetGroupUpgradeStatusRequest { group_id }: GetGroupUpgradeStatusRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let Some((node_identity, _)) = self.node_group_identity() else {
                bail!("node has no group identity configured");
            };
            if !group_store::check_group_membership(&self.datastore, &group_id, &node_identity)? {
                bail!("node is not a member of group '{group_id:?}'");
            }
            group_store::load_group_upgrade(&self.datastore, &group_id)
                .map(|opt| opt.map(Into::into))
        })();
        ActorResponse::reply(result)
    }
}
