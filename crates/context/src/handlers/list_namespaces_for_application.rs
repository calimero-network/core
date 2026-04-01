use actix::{ActorResponse, Handler, Message};
use calimero_context_config::types::ContextGroupId;
use calimero_context_primitives::group::{ListNamespacesForApplicationRequest, NamespaceSummary};

use crate::group_store;
use crate::ContextManager;

impl Handler<ListNamespacesForApplicationRequest> for ContextManager {
    type Result = ActorResponse<Self, <ListNamespacesForApplicationRequest as Message>::Result>;

    fn handle(
        &mut self,
        ListNamespacesForApplicationRequest {
            application_id,
            offset,
            limit,
        }: ListNamespacesForApplicationRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let entries = group_store::enumerate_all_groups(&self.datastore, 0, usize::MAX)?;

            let mut namespaces = Vec::new();
            for (group_id_bytes, meta) in entries {
                if meta.target_application_id != application_id {
                    continue;
                }

                let group_id = ContextGroupId::from(group_id_bytes);

                if group_store::get_parent_group(&self.datastore, &group_id)?.is_some() {
                    continue;
                }

                let Some((node_identity, _)) = self.node_namespace_identity(&group_id) else {
                    continue;
                };
                if !group_store::check_group_membership(&self.datastore, &group_id, &node_identity)?
                {
                    continue;
                }

                let alias = group_store::get_group_alias(&self.datastore, &group_id)
                    .ok()
                    .flatten();
                let member_count =
                    group_store::count_group_members(&self.datastore, &group_id).unwrap_or(0);
                let contexts =
                    group_store::enumerate_group_contexts(&self.datastore, &group_id, 0, usize::MAX)
                        .unwrap_or_default();
                let children =
                    group_store::enumerate_child_groups(&self.datastore, &group_id)
                        .unwrap_or_default();

                namespaces.push(NamespaceSummary {
                    namespace_id: group_id,
                    app_key: meta.app_key.into(),
                    target_application_id: meta.target_application_id,
                    upgrade_policy: meta.upgrade_policy,
                    created_at: meta.created_at,
                    alias,
                    member_count,
                    context_count: contexts.len(),
                    subgroup_count: children.len(),
                });
            }

            let total = namespaces.len();
            let start = offset.min(total);
            let end = (start + limit).min(total);
            Ok(namespaces[start..end].to_vec())
        })();

        ActorResponse::reply(result)
    }
}
