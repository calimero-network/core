use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::UpdateMemberRoleRequest;
use calimero_node_primitives::sync::GroupMutationKind;
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
        let node_identity = self.node_group_identity();

        // Resolve requester: use provided value or fall back to node group identity
        let requester = match requester {
            Some(pk) => pk,
            None => match node_identity {
                Some((pk, _)) => pk,
                None => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "requester not provided and node has no configured group identity"
                    )))
                }
            },
        };

        // Resolve signing_key from node identity key
        let node_sk = node_identity.map(|(_, sk)| sk);
        let signing_key = node_sk;

        if let Err(err) = (|| -> eyre::Result<()> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group '{group_id:?}' not found");
            }

            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;

            // Auto-store signing key if provided
            if let Some(ref sk) = signing_key {
                let _ = group_store::store_group_signing_key(
                    &self.datastore,
                    &group_id,
                    &requester,
                    sk,
                );
            }

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
        })() {
            return ActorResponse::reply(Err(err));
        }

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();

        ActorResponse::r#async(
            async move {
                let contexts =
                    group_store::enumerate_group_contexts(&datastore, &group_id, 0, usize::MAX)?;
                let _ = node_client
                    .broadcast_group_mutation(
                        &contexts,
                        group_id.to_bytes(),
                        GroupMutationKind::MemberRoleUpdated,
                    )
                    .await;
                Ok(())
            }
            .into_actor(self),
        )
    }
}
