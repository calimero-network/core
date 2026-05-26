#![allow(deprecated)] // #2303: per-file Repository migration deferred to follow-up

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::SetMemberAutoFollowRequest;
use calimero_context_client::local_governance::GroupOp;

use crate::governance_broadcast::ObserveDelivery;
use crate::{group_store, ContextManager};

impl Handler<SetMemberAutoFollowRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetMemberAutoFollowRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetMemberAutoFollowRequest {
            group_id,
            target,
            auto_follow_contexts,
            auto_follow_subgroups,
            requester,
        }: SetMemberAutoFollowRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Admin-or-self is enforced inside the apply path
        // (`GroupOp::MemberSetAutoFollow`), so don't require admin here —
        // a non-admin self-setter must be allowed through preflight.
        let preflight = match self.governance_preflight(&group_id, requester, false) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        // Surface the membership check up-front for a clearer error than the
        // generic apply-path "target is not a member" bail.
        if group_store::get_group_member_role(&self.datastore, &group_id, &target)
            .ok()
            .flatten()
            .is_none()
        {
            return ActorResponse::reply(Err(eyre::eyre!(
                "target is not a member of group '{group_id:?}'"
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
                    GroupOp::MemberSetAutoFollow {
                        target,
                        auto_follow_contexts,
                        auto_follow_subgroups,
                    },
                )
                .await?;
                report.observe("set_member_auto_follow", "MemberSetAutoFollow");
                tracing::info!(
                    ?group_id,
                    %target,
                    auto_follow_contexts,
                    auto_follow_subgroups,
                    "member auto-follow flags updated"
                );
                Ok(())
            }
            .into_actor(self),
        )
    }
}
