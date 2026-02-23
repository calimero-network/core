use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::AddGroupMembersRequest;
use eyre::bail;
use tracing::info;

use crate::group_store;
use crate::ContextManager;

impl Handler<AddGroupMembersRequest> for ContextManager {
    type Result = ActorResponse<Self, <AddGroupMembersRequest as Message>::Result>;

    fn handle(
        &mut self,
        AddGroupMembersRequest {
            group_id,
            members,
            requester,
        }: AddGroupMembersRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group not found");
            }

            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;

            for (identity, role) in &members {
                group_store::add_group_member(&self.datastore, &group_id, identity, role.clone())?;
            }

            info!(?group_id, count = members.len(), %requester, "members added to group");

            Ok(())
        })();

        ActorResponse::reply(result)
    }
}
