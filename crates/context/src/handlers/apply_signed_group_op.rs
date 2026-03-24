use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::messages::ApplySignedGroupOpRequest;

use crate::group_store;
use crate::ContextManager;

impl Handler<ApplySignedGroupOpRequest> for ContextManager {
    type Result = ActorResponse<Self, <ApplySignedGroupOpRequest as Message>::Result>;

    fn handle(
        &mut self,
        ApplySignedGroupOpRequest { op }: ApplySignedGroupOpRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let datastore = self.datastore.clone();
        ActorResponse::r#async(
            async move { group_store::apply_local_signed_group_op(&datastore, &op) }
                .into_actor(self),
        )
    }
}
