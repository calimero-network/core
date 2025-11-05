use actix::{Handler, Message};
use calimero_context_primitives::messages::SyncRequest;

use crate::ContextManager;

impl Handler<SyncRequest> for ContextManager {
    type Result = <SyncRequest as Message>::Result;

    fn handle(
        &mut self,
        SyncRequest {
            context_id,
            application_id,
        }: SyncRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Update application ID in cache via repository
        let _updated = self
            .repository
            .update_application_id(&context_id, application_id);
    }
}
