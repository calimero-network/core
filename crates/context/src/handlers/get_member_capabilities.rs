use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::{
    GetMemberCapabilitiesRequest, GetMemberCapabilitiesResponse,
};
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

            if group_store::get_group_member_role(&self.datastore, &group_id, &member)?.is_none() {
                bail!("identity is not a member of group '{group_id:?}'");
            }

            let capabilities =
                group_store::get_member_capability(&self.datastore, &group_id, &member)?
                    .unwrap_or(0);

            Ok(GetMemberCapabilitiesResponse { capabilities })
        })();

        ActorResponse::reply(result)
    }
}
