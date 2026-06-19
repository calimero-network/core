use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::GetGroupUpgradeStatusRequest;
use calimero_governance_store::UpgradesRepository;
use eyre::bail;

use crate::ContextManager;

impl Handler<GetGroupUpgradeStatusRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetGroupUpgradeStatusRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetGroupUpgradeStatusRequest { group_id }: GetGroupUpgradeStatusRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let Some((node_identity, _)) = self.node_namespace_identity(&group_id) else {
                bail!("node has no group identity configured");
            };
            if !crate::scope_projection::ScopeProjections::member_now_checked(
                &self.datastore,
                &group_id,
                &node_identity,
            )? {
                bail!("node is not a member of group '{group_id:?}'");
            }
            UpgradesRepository::new(&self.datastore)
                .load(&group_id)
                .map(|opt| opt.map(Into::into))
        })();
        ActorResponse::reply(result)
    }
}
