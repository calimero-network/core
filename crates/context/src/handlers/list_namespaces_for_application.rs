use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{ListNamespacesForApplicationRequest, NamespaceSummary};
use calimero_governance_store::{ForgottenNamespaces, MetadataRepository};

use crate::handlers::list_namespaces::{
    collect_namespace_summaries, namespace_rows_for_applications, paginate_namespaces,
};
use crate::ContextManager;
use calimero_governance_store;

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
            let entries = namespace_rows_for_applications(&self.datastore, &[application_id])?;
            let mut namespaces = collect_namespace_summaries(
                entries,
                |group_id| self.node_signing_key(group_id),
                |group_id, meta, node_identity| {
                    MetadataRepository::new(&self.datastore).build_namespace_summary(
                        group_id,
                        meta,
                        node_identity,
                    )
                },
            )?;
            namespaces.extend(
                ForgottenNamespaces::new(&self.datastore)
                    .list()?
                    .into_iter()
                    .filter(|removed| removed.application_id == Some(application_id))
                    .map(|removed| NamespaceSummary {
                        namespace_id: removed.namespace_id,
                        bytecode_id: [0; 32].into(),
                        target_application_id: application_id,
                        target_release: None,
                        created_at: 0,
                        name: removed.name,
                        member_count: 0,
                        context_count: 0,
                        subgroup_count: 0,
                        forgotten: true,
                    }),
            );
            Ok(paginate_namespaces(&namespaces, offset, limit))
        })();

        ActorResponse::reply(result)
    }
}
