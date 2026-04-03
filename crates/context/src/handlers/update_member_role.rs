use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::UpdateMemberRoleRequest;
use calimero_context_primitives::local_governance::GroupOp;
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
        let preflight = match self.governance_preflight(&group_id, requester, true) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        if let Err(err) = (|| -> eyre::Result<()> {
            let Some(current_role) =
                group_store::get_group_member_role(&self.datastore, &group_id, &identity)?
            else {
                bail!("identity is not a member of group '{group_id:?}'");
            };

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

        let Some(current_role) =
            group_store::get_group_member_role(&self.datastore, &group_id, &identity)
                .ok()
                .flatten()
        else {
            return ActorResponse::reply(Err(eyre::eyre!(
                "identity is not a member of group '{group_id:?}'"
            )));
        };

        if current_role == new_role {
            return ActorResponse::reply(Ok(()));
        }

        let datastore = preflight.datastore.clone();
        let node_client = preflight.node_client.clone();
        let sk = preflight.signer_sk();

        ActorResponse::r#async(
            async move {
                group_store::sign_apply_and_publish(
                    &datastore,
                    &node_client,
                    &group_id,
                    &sk,
                    GroupOp::MemberRoleSet {
                        member: identity,
                        role: new_role,
                    },
                )
                .await?;
                Ok(())
            }
            .into_actor(self),
        )
    }
}
