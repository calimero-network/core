use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::RemoveGroupMembersRequest;
use calimero_primitives::context::GroupMemberRole;
use eyre::bail;
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

            let admin_count = group_store::count_group_admins(&self.datastore, &group_id)?;
            let admins_being_removed = members
                .iter()
                .filter(|id| {
                    group_store::get_group_member_role(&self.datastore, &group_id, id)
                        .ok()
                        .flatten()
                        == Some(GroupMemberRole::Admin)
                })
                .count();

            if admin_count <= admins_being_removed {
                bail!("cannot remove all admins from group '{group_id:?}': at least one admin must remain");
            }

            for identity in &members {
                group_store::remove_group_member(&self.datastore, &group_id, identity)?;
            }

            info!(?group_id, count = members.len(), %requester, "members removed from group");

            Ok(())
        })();

        ActorResponse::reply(result)
    }
}
