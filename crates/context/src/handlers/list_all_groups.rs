use actix::{ActorResponse, Handler, Message};
use calimero_context_config::types::ContextGroupId;
use calimero_context_primitives::group::{GroupSummary, ListAllGroupsRequest};

use crate::group_store;
use crate::ContextManager;

impl Handler<ListAllGroupsRequest> for ContextManager {
    type Result = ActorResponse<Self, <ListAllGroupsRequest as Message>::Result>;

    fn handle(
        &mut self,
        ListAllGroupsRequest { offset, limit }: ListAllGroupsRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let Some((node_identity, _)) = self.node_group_identity() else {
                return Ok(vec![]);
            };

            let entries = group_store::enumerate_all_groups(&self.datastore, offset, limit)?;

            let mut summaries = Vec::with_capacity(entries.len());
            for (group_id_bytes, meta) in entries {
                let group_id = ContextGroupId::from(group_id_bytes);
                if group_store::check_group_membership(&self.datastore, &group_id, &node_identity)?
                {
                    summaries.push(GroupSummary {
                        group_id,
                        app_key: meta.app_key.into(),
                        target_application_id: meta.target_application_id,
                        upgrade_policy: meta.upgrade_policy,
                        created_at: meta.created_at,
                    });
                }
            }
            Ok(summaries)
        })();

        ActorResponse::reply(result)
    }
}
