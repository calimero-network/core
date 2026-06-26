use calimero_governance_store::NamespaceRepository;
use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::SetSubgroupVisibilityRequest;
use calimero_context_client::local_governance::GroupOp;
use tracing::warn;

use crate::ContextManager;
use calimero_governance_store::governance_broadcast::ObserveDelivery;

impl Handler<SetSubgroupVisibilityRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetSubgroupVisibilityRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetSubgroupVisibilityRequest {
            group_id,
            subgroup_visibility,
            requester,
        }: SetSubgroupVisibilityRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Setting visibility on the namespace root is a no-op for
        // inheritance: `check_group_membership_path` only consults a
        // subgroup's visibility to decide whether to keep walking
        // *upward* from it, and the root has no parent. Warn so
        // operators notice the meaningless call rather than silently
        // accepting it; we don't reject because existing workflows
        // (including e2e suites and likely external clients) issue
        // the call as a harmless setup step.
        if let Ok(ns_id) = NamespaceRepository::new(&self.datastore).resolve(&group_id) {
            if ns_id == group_id {
                warn!(
                    group_id = %hex::encode(group_id.to_bytes()),
                    "subgroup_visibility set on the namespace root has no effect on \
                     inheritance — the root is itself the inheritance boundary; only \
                     descendant subgroups observe this setting via the parent walk"
                );
            }
        }

        // Preflight without the admin gate (resolves the requester + signing
        // key, checks the group exists), then require admin **or**
        // `CAN_MANAGE_VISIBILITY` — re-checked on every peer in
        // `GroupSettingsService::set_subgroup_visibility`.
        let preflight = match self.governance_preflight(&group_id, requester, false) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        if let Err(err) =
            calimero_governance_store::PermissionChecker::new(&self.datastore, group_id)
                .require_can_manage_visibility(&preflight.requester)
        {
            return ActorResponse::reply(Err(err));
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
                    GroupOp::SubgroupVisibilitySet { mode: subgroup_visibility },
                )
                .await?;
                report.observe("set_subgroup_visibility", "SubgroupVisibilitySet");
                Ok(())
            }
            .into_actor(self),
        )
    }
}
