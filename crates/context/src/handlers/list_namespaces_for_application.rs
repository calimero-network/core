use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::ListNamespacesForApplicationRequest;

use crate::group_store;
use crate::handlers::list_namespaces::{collect_namespace_summaries, paginate_namespaces};
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
            let namespaces = collect_namespace_summaries(
                entries,
                Some(application_id),
                |group_id| self.node_namespace_identity(group_id),
                |group_id, meta, node_identity| {
                    group_store::build_namespace_summary(
                        &self.datastore,
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
