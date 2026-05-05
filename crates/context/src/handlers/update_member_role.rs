use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::UpdateMemberRoleRequest;
use calimero_context_client::local_governance::GroupOp;
use calimero_primitives::context::GroupMemberRole;

use crate::governance_broadcast::observe_handler_delivery;
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
        // Admin check first — prevents non-admins from probing role status.
        let preflight = match self.governance_preflight(&group_id, requester, true) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        // ReadOnlyTee is only assigned via TEE attestation, not manually.
        if new_role == GroupMemberRole::ReadOnlyTee {
            return ActorResponse::reply(Err(eyre::eyre!(
                "ReadOnlyTee role can only be assigned via TEE attestation admission"
            )));
        }

        // Single DB read for current role.
        let current_role =
            match group_store::get_group_member_role(&self.datastore, &group_id, &identity) {
                Ok(Some(role)) => role,
                Ok(None) => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "identity is not a member of group '{group_id:?}'"
                    )));
                }
                Err(err) => return ActorResponse::reply(Err(err)),
            };

        if current_role == new_role {
            return ActorResponse::reply(Ok(()));
        }

        if current_role == GroupMemberRole::Admin && new_role == GroupMemberRole::Member {
            match group_store::count_group_admins(&self.datastore, &group_id) {
                Ok(count) if count <= 1 => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "cannot demote the last admin of group '{group_id:?}'"
                    )));
                }
                Err(err) => return ActorResponse::reply(Err(err)),
                _ => {}
            }
        }

        // Inline the sign+publish to avoid a second governance_preflight call.
        let datastore = preflight.datastore.clone();
        let node_client = preflight.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);
        let sk = preflight.signer_sk();

        ActorResponse::r#async(
            async move {
                let report = group_store::sign_apply_and_publish(
                    &datastore,
                    &node_client,
                    &ack_router,
                    &group_id,
                    &sk,
                    GroupOp::MemberRoleSet {
                        member: identity,
                        role: new_role,
                    },
                )
                .await?;
                if let Some(report) = report.as_ref() {
                    observe_handler_delivery("update_member_role", "MemberRoleSet", report);
                }
                tracing::info!(?group_id, ?identity, "member role updated");
                Ok(())
            }
            .into_actor(self),
        )
    }
}
