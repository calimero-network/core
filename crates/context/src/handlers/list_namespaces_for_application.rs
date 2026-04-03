use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{ListNamespacesForApplicationRequest, NamespaceSummary};
use calimero_context_config::types::ContextGroupId;

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

                let Some((node_identity, _)) = self.node_namespace_identity(&group_id) else {
                    continue;
                };

                if let Some(ns) = group_store::build_namespace_summary(
                    &self.datastore,
                    &group_id,
                    &meta,
                    &node_identity,
                )? {
                    namespaces.push(ns);
                }
            }

            let total = namespaces.len();
            let start = offset.min(total);
            let end = (start + limit).min(total);
            Ok(namespaces[start..end].to_vec())
        })();

        ActorResponse::reply(result)
    }
}
