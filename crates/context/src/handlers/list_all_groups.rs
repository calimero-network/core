use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{GroupSummary, ListAllGroupsRequest};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{MetaRepository, MetadataRepository};

use crate::ContextManager;
use calimero_governance_store;

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
                // Skip (don't abort the whole listing) on a per-group membership
                // error — mirrors the `node_namespace_identity` miss above. A
                // transient store fault on one group must not discard every group
                // already accumulated.
                let is_member = match crate::scope_projection::ScopeProjections::member_now_checked(
                    &self.datastore,
                    &group_id,
                    &node_identity,
                ) {
                    Ok(m) => m,
                    Err(err) => {
                        tracing::warn!(group_id = ?group_id, %err, "list_all_groups: membership check failed; skipping group");
                        continue;
                    }
                };
                if is_member {
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
