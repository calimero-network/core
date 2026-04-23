use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::messages::NamespacePendingOpCountRequest;

use crate::ContextManager;

impl Handler<NamespacePendingOpCountRequest> for ContextManager {
    type Result = ActorResponse<Self, <NamespacePendingOpCountRequest as Message>::Result>;

    fn handle(
        &mut self,
        NamespacePendingOpCountRequest { namespace_id }: NamespacePendingOpCountRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let dag = self.get_or_create_namespace_dag(&namespace_id);

        ActorResponse::r#async(
            async move {
                let dag = dag.lock().await;
                Ok(dag.stats().pending_deltas)
            }
            .into_actor(self),
        )
    }
}
