use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{GetMemberCapabilitiesRequest, GetMemberCapabilitiesResponse};
use calimero_governance_store::{MembershipRepository, MetaRepository};
use eyre::bail;

use crate::ContextManager;
use calimero_governance_store;

impl Handler<GetMemberCapabilitiesRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetMemberCapabilitiesRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetMemberCapabilitiesRequest { group_id, member }: GetMemberCapabilitiesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| -> eyre::Result<GetMemberCapabilitiesResponse> {
            if MetaRepository::new(&self.datastore)
                .load(&group_id)?
                .is_none()
            {
                bail!("group '{group_id:?}' not found");
            }

            let Some(capabilities) = MembershipRepository::new(&self.datastore)
                .effective_capabilities(&group_id, &member)?
            else {
                bail!("identity is not a member of group '{group_id:?}'");
            };

            Ok(GetMemberCapabilitiesResponse { capabilities })
        })();

        ActorResponse::reply(result)
    }
}
