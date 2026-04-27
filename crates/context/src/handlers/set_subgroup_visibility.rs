use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::SetSubgroupVisibilityRequest;
use calimero_context_client::local_governance::GroupOp;

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
        // Visibility on the namespace root is a silent no-op: the
        // inheritance walk in `check_group_membership_path` only
        // consults a subgroup's visibility to decide whether to keep
        // walking *upward* from it. The root has no parent and is
        // itself the inheritance boundary, so its `subgroup_visibility`
        // setting never affects any descendant's authorization.
        // Surface the no-op explicitly instead of accepting + storing
        // a meaningless value.
        match group_store::resolve_namespace(&self.datastore, &group_id) {
            Ok(ns_id) if ns_id == group_id => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "subgroup_visibility cannot be set on the namespace root \
                     (group_id == namespace_id); visibility only governs \
                     inheritance into descendant subgroups"
                )));
            }
            Ok(_) => {}
            Err(err) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "failed to resolve namespace for visibility check: {err}"
                )));
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
