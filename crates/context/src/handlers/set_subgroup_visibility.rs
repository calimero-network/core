use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::SetSubgroupVisibilityRequest;
use calimero_context_client::local_governance::GroupOp;
use tracing::warn;

use crate::{group_store, ContextManager};

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
        if let Ok(ns_id) = group_store::resolve_namespace(&self.datastore, &group_id) {
            if ns_id == group_id {
                warn!(
                    group_id = %hex::encode(group_id.to_bytes()),
                    "subgroup_visibility set on the namespace root has no effect on \
                     inheritance — the root is itself the inheritance boundary; only \
                     descendant subgroups observe this setting via the parent walk"
                );
            }
        }

        let mode_u8 = match subgroup_visibility {
            calimero_context_config::VisibilityMode::Open => 0u8,
            calimero_context_config::VisibilityMode::Restricted => 1u8,
        };

        self.sign_and_publish_group_op(
            &group_id,
            requester,
            true,
            GroupOp::SubgroupVisibilitySet { mode: mode_u8 },
        )
    }
}
