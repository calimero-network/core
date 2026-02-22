use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::RemoveGroupMembersRequest;
use tracing::info;

use crate::group_store;
use crate::ContextManager;

impl Handler<RemoveGroupMembersRequest> for ContextManager {
    type Result = ActorResponse<Self, <RemoveGroupMembersRequest as Message>::Result>;

    fn handle(
        &mut self,
        RemoveGroupMembersRequest {
            group_id,
            members,
            requester,
        }: RemoveGroupMembersRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;

            for identity in &members {
                group_store::remove_group_member(&self.datastore, &group_id, identity)?;
            }

            info!(?group_id, count = members.len(), %requester, "members removed from group");

            Ok(())
        })();

        ActorResponse::reply(result)
    }
}
