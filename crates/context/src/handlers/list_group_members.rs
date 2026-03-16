use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::{GroupMemberEntry, ListGroupMembersRequest};
use eyre::bail;

use crate::group_store;
use crate::ContextManager;

impl Handler<ListGroupMembersRequest> for ContextManager {
    type Result = ActorResponse<Self, <ListGroupMembersRequest as Message>::Result>;

    fn handle(
        &mut self,
        ListGroupMembersRequest {
            group_id,
            offset,
            limit,
        }: ListGroupMembersRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let Some((node_identity, _)) = self.node_group_identity() else {
                bail!("node has no group identity configured");
            };
            if !group_store::check_group_membership(&self.datastore, &group_id, &node_identity)? {
                bail!("node is not a member of group '{group_id:?}'");
            }

            let members =
                group_store::list_group_members(&self.datastore, &group_id, offset, limit)?;

            let entries = members
                .into_iter()
                .map(|(identity, role)| {
                    let alias =
                        group_store::get_member_alias(&self.datastore, &group_id, &identity)
                            .ok()
                            .flatten();
                    GroupMemberEntry {
                        identity,
                        role,
                        alias,
                    }
                })
                .collect();
            Ok(entries)
        })();

        ActorResponse::reply(result)
    }
}
