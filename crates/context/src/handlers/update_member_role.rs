use calimero_governance_store::MembershipRepository;
use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::UpdateMemberRoleRequest;
use calimero_context_client::local_governance::GroupOp;
use calimero_primitives::context::GroupMemberRole;

use crate::ContextManager;
use calimero_governance_store::governance_broadcast::ObserveDelivery;

impl Handler<UpdateMemberRoleRequest> for ContextManager {
    type Result = ActorResponse<Self, <UpdateMemberRoleRequest as Message>::Result>;

    fn handle(
        &mut self,
        UpdateMemberRoleRequest {
            group_id,
            identity,
            new_role,
        }: UpdateMemberRoleRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Admin check first — prevents non-admins from probing role status.
        let preflight = match self.governance_preflight(&group_id, true) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        // A TEE role is only assigned via TEE attestation, not manually. The
        // replica/relay conversion is the namespace's admission mode: setting
        // the policy's mode converts the TEEs already admitted.
        if new_role.is_tee() {
            return ActorResponse::reply(Err(eyre::eyre!(
                "TEE roles (ReadOnlyTee, RelayTee) can only be assigned via TEE attestation \
                 admission; to switch TEEs between replica and relay, set the namespace's \
                 TEE admission policy mode"
            )));
        }

        // Single DB read for current role.
        let current_role =
            match MembershipRepository::new(&self.datastore).role_of(&group_id, &identity) {
                Ok(Some(role)) => role,
                Ok(None) => {
                    // The member the CALLER named, not this node. 404 matches
                    // `get_member_capabilities`: the caller asked about somebody
                    // who is not in this group.
                    return ActorResponse::reply(Err(
                        calimero_governance_store::MembershipError::MemberNotFound {
                            group_id: group_id.to_string(),
                            member: identity.to_string(),
                        }
                        .into(),
                    ));
                }
                Err(err) => return ActorResponse::reply(Err(err)),
            };

        if current_role == new_role {
            return ActorResponse::reply(Ok(()));
        }

        // An attested TEE stays in the TEE roles; the apply refuses the op on
        // every peer, so refuse it here, before signing, with the same error.
        // A TEE is removed, not demoted.
        if let Err(err) =
            calimero_governance_store::MembershipPolicy::require_tee_row_keeps_tee_role(
                &identity,
                &current_role,
                &new_role,
            )
        {
            return ActorResponse::reply(Err(err.into()));
        }

        if current_role == GroupMemberRole::Admin && new_role == GroupMemberRole::Member {
            match MembershipRepository::new(&self.datastore).count_admins(&group_id) {
                Ok(count) if count <= 1 => {
                    return ActorResponse::reply(Err(
                        calimero_governance_store::MembershipError::LastAdminDemotion.into(),
                    ));
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
                let report = calimero_governance_store::sign_apply_and_publish(
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
                report.observe("update_member_role", "MemberRoleSet");
                tracing::debug!(?group_id, ?identity, "member role updated");
                Ok(())
            }
            .into_actor(self),
        )
    }
}
