use crate::group_store::{MembershipRepository, MetaRepository, MetadataRepository};
use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{GroupSummary, ListAllGroupsRequest};
use calimero_context_config::types::ContextGroupId;

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
            let entries = MetaRepository::new(&self.datastore).enumerate_all(offset, limit)?;

            let mut summaries = Vec::with_capacity(entries.len());
            for (group_id_bytes, meta) in entries {
                let group_id = ContextGroupId::from(group_id_bytes);
                let Some((node_identity, _)) = self.node_namespace_identity(&group_id) else {
                    continue;
                };
                if MembershipRepository::new(&self.datastore)
                    .is_member(&group_id, &node_identity)?
                {
                    let name = MetadataRepository::new(&self.datastore)
                        .group_metadata(&group_id)
                        .ok()
                        .flatten()
                        .and_then(|r| r.name);
                    summaries.push(GroupSummary {
                        group_id,
                        app_key: meta.app_key.into(),
                        target_application_id: meta.target_application_id,
                        upgrade_policy: meta.upgrade_policy,
                        created_at: meta.created_at,
                        name,
                    });
                }
            }
            Ok(summaries)
        })();

        ActorResponse::reply(result)
    }
}
