//! Serve an outbound request a member's node forwarded because it names a
//! `{{sealed:…}}` secret only a TEE of the namespace can open. See
//! [`crate::outbound_forward`].

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::messages::ForwardedOutboundRequest;

use crate::outbound_forward::answer_forwarded;
use crate::ContextManager;

impl Handler<ForwardedOutboundRequest> for ContextManager {
    type Result = ActorResponse<Self, <ForwardedOutboundRequest as Message>::Result>;

    fn handle(
        &mut self,
        ForwardedOutboundRequest {
            context,
            requester,
            responder,
            request,
        }: ForwardedOutboundRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let service = self.outbound.clone();
        let context_client = self.context_client.clone();
        let node_client = self.node_client.clone();
        let datastore = self.datastore.clone();
        let scope_projections = Arc::clone(&self.scope_projections);
        // The request goes out over HTTPS and waits for its answer, so it runs
        // on the blocking pool rather than in the actor.
        let answer = async move {
            let answer = tokio::task::spawn_blocking(move || {
                answer_forwarded(
                    service.as_ref(),
                    &context_client,
                    &node_client,
                    &datastore,
                    &scope_projections,
                    context,
                    requester,
                    responder,
                    &request,
                )
            })
            .await?;
            Ok(borsh::to_vec(&answer)?)
        };
        ActorResponse::r#async(answer.into_actor(self))
    }
}
