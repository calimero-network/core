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
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group '{group_id:?}' not found");
            }

            let members =
                group_store::list_group_members(&self.datastore, &group_id, offset, limit)?;

            Ok(members
                .into_iter()
                .map(|(identity, role)| GroupMemberEntry { identity, role })
                .collect())
        })();

        ActorResponse::reply(result)
    }
}
