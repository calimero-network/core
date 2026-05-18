use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{GetMemberCapabilitiesRequest, GetMemberCapabilitiesResponse};
use eyre::bail;

use crate::group_store;
use crate::ContextManager;

impl Handler<GetMemberCapabilitiesRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetMemberCapabilitiesRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetMemberCapabilitiesRequest { group_id, member }: GetMemberCapabilitiesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| -> eyre::Result<GetMemberCapabilitiesResponse> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group '{group_id:?}' not found");
            }

            let Some(capabilities) = group_store::get_effective_member_capabilities(
                &self.datastore,
                &group_id,
                &member,
            )?
            else {
                bail!("identity is not a member of group '{group_id:?}'");
            };

            Ok(GetMemberCapabilitiesResponse { capabilities })
        })();

        ActorResponse::reply(result)
    }
}
