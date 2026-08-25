use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::ListNamespacesForApplicationRequest;
use calimero_governance_store::MetadataRepository;

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
            let namespaces = collect_namespace_summaries(
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
            Ok(paginate_namespaces(&namespaces, offset, limit))
        })();

        ActorResponse::reply(result)
    }
}
