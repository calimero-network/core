use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::UpdateMemberRoleRequest;
use calimero_primitives::context::GroupMemberRole;
use eyre::bail;

use crate::group_store;
use crate::ContextManager;

impl Handler<UpdateMemberRoleRequest> for ContextManager {
    type Result = ActorResponse<Self, <UpdateMemberRoleRequest as Message>::Result>;

    fn handle(
        &mut self,
        UpdateMemberRoleRequest {
            group_id,
            identity,
            new_role,
            requester,
        }: UpdateMemberRoleRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group '{group_id:?}' not found");
            }

            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;
            group_store::require_group_signing_key(&self.datastore, &group_id, &requester)?;

            let Some(current_role) =
                group_store::get_group_member_role(&self.datastore, &group_id, &identity)?
            else {
                bail!("identity is not a member of group '{group_id:?}'");
            };

            if current_role == new_role {
                return Ok(());
            }

            if current_role == GroupMemberRole::Admin && new_role == GroupMemberRole::Member {
                let admin_count = group_store::count_group_admins(&self.datastore, &group_id)?;
                if admin_count <= 1 {
                    bail!("cannot demote the last admin of group '{group_id:?}'");
                }
            }

            group_store::add_group_member(&self.datastore, &group_id, &identity, new_role)?;

            Ok(())
        })();

        ActorResponse::reply(result)
    }
}
