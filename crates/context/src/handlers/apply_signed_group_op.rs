use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::messages::ApplySignedGroupOpRequest;

use crate::ContextManager;
use calimero_governance_store;

impl Handler<ApplySignedGroupOpRequest> for ContextManager {
    type Result = ActorResponse<Self, <ApplySignedGroupOpRequest as Message>::Result>;

    fn handle(
        &mut self,
        ApplySignedGroupOpRequest { op }: ApplySignedGroupOpRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let datastore = self.datastore.clone();

        ActorResponse::r#async(
            async move {
                match calimero_governance_store::apply_local_signed_group_op(&datastore, &op) {
                    Ok(()) => Ok(true),
                    Err(e) => {
                        tracing::debug!(%e, "failed to apply group op");
                        Err(e)
                    }
                }
            }
            .into_actor(self),
        )
    }
}
