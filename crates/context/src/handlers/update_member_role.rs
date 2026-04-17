use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::UpdateMemberRoleRequest;
use calimero_context_client::local_governance::GroupOp;
use calimero_primitives::context::GroupMemberRole;
use eyre::bail;

use crate::{group_store, ContextManager};

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
        // Validate: member exists, not demoting last admin, not a no-op
        if let Err(err) = (|| -> eyre::Result<()> {
            let Some(current_role) =
                group_store::get_group_member_role(&self.datastore, &group_id, &identity)?
            else {
                bail!("identity is not a member of group '{group_id:?}'");
            };

            if current_role == new_role {
                return Ok(()); // no-op handled below
            }

            if current_role == GroupMemberRole::Admin && new_role == GroupMemberRole::Member {
                let admin_count = group_store::count_group_admins(&self.datastore, &group_id)?;
                if admin_count <= 1 {
                    bail!("cannot demote the last admin of group '{group_id:?}'");
                }
            }

            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        // Check for no-op (same role)
        let current_role =
            group_store::get_group_member_role(&self.datastore, &group_id, &identity)
                .ok()
                .flatten();
        if current_role == Some(new_role.clone()) {
            return ActorResponse::reply(Ok(()));
        }

        self.sign_and_publish_group_op(
            &group_id,
            requester,
            true,
            GroupOp::MemberRoleSet {
                member: identity,
                role: new_role,
            },
        )
    }
}
