use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::ListNamespacesForApplicationRequest;
use calimero_governance_store::{MetaRepository, MetadataRepository};

use crate::handlers::list_namespaces::{collect_namespace_summaries, paginate_namespaces};
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
            let entries = MetaRepository::new(&self.datastore).enumerate_all(0, usize::MAX)?;
            let namespaces = collect_namespace_summaries(
                entries,
                Some(application_id),
                |group_id| self.node_namespace_identity(group_id),
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
