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
            let entries = group_store::enumerate_all_groups(&self.datastore, offset, limit)?;

            Ok(entries
                .into_iter()
                .map(|(group_id_bytes, meta)| GroupSummary {
                    group_id: ContextGroupId::from(group_id_bytes),
                    app_key: meta.app_key.into(),
                    target_application_id: meta.target_application_id,
                    upgrade_policy: meta.upgrade_policy,
                    created_at: meta.created_at,
                })
                .collect())
        })();

        ActorResponse::reply(result)
    }
}
