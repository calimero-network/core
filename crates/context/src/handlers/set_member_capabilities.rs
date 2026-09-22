use calimero_governance_store::MembershipRepository;
use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::SetMemberCapabilitiesRequest;
use calimero_context_client::local_governance::GroupOp;

use crate::ContextManager;
use calimero_governance_store::governance_broadcast::ObserveDelivery;

impl Handler<SetMemberCapabilitiesRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetMemberCapabilitiesRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetMemberCapabilitiesRequest {
            group_id,
            member,
            capabilities,
        }: SetMemberCapabilitiesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Admin check first — prevents non-admins from probing membership.
        let preflight = match self.governance_preflight(&group_id, true) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        if MembershipRepository::new(&self.datastore)
            .role_of(&group_id, &member)
            .ok()
            .flatten()
            .is_none()
        {
            // The member the CALLER named, not this node. 404 matches
            // `get_member_capabilities`: the caller asked about somebody
            // who is not in this group.
            return ActorResponse::reply(Err(
                calimero_governance_store::MembershipError::MemberNotFound {
                    group_id: format!("{group_id:?}"),
                    member: member.to_string(),
                }
                .into(),
            ));
        }

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
                    GroupOp::MemberCapabilitySet {
                        member,
                        capabilities:
                            calimero_context_config::MemberCapabilities::from_bits_truncate(
                                capabilities,
                            ),
                    },
                )
                .await?;
                report.observe("set_member_capabilities", "MemberCapabilitySet");
                tracing::debug!(?group_id, %member, capabilities, "member capabilities updated");
                Ok(())
            }
            .into_actor(self),
        )
    }
}
