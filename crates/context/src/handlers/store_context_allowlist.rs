use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::StoreContextAllowlistRequest;

use crate::{group_store, ContextManager};

impl Handler<StoreContextAllowlistRequest> for ContextManager {
    type Result = ActorResponse<Self, <StoreContextAllowlistRequest as Message>::Result>;

    fn handle(
        &mut self,
        StoreContextAllowlistRequest {
            group_id,
            context_id,
            members,
        }: StoreContextAllowlistRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| -> eyre::Result<()> {
            group_store::clear_context_allowlist(&self.datastore, &group_id, &context_id)?;
            for member in &members {
                group_store::add_to_context_allowlist(
                    &self.datastore,
                    &group_id,
                    &context_id,
                    member,
                )?;
            }
            Ok(())
        })();
        ActorResponse::reply(result)
    }
}
