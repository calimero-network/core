use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::SetMemberCapabilitiesRequest;
use calimero_context_client::local_governance::GroupOp;

use crate::governance_broadcast::ObserveDelivery;
use crate::{group_store, ContextManager};

impl Handler<SetMemberCapabilitiesRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetMemberCapabilitiesRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetMemberCapabilitiesRequest {
            group_id,
            member,
            capabilities,
            requester,
        }: SetMemberCapabilitiesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Admin check first — prevents non-admins from probing membership.
        let preflight = match self.governance_preflight(&group_id, requester, true) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        if group_store::get_group_member_role(&self.datastore, &group_id, &member)
            .ok()
            .flatten()
            .is_none()
        {
            return ActorResponse::reply(Err(eyre::eyre!(
                "identity is not a member of group '{group_id:?}'"
            )));
        }

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
                    GroupOp::MemberCapabilitySet {
                        member,
                        capabilities,
                    },
                )
                .await?;
                report.observe("set_member_capabilities", "MemberCapabilitySet");
                tracing::info!(?group_id, %member, capabilities, "member capabilities updated");
                Ok(())
            }
            .into_actor(self),
        )
    }
}
