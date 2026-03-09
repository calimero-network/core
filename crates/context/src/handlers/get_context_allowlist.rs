use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::GetContextAllowlistRequest;
use calimero_primitives::identity::PublicKey;
use eyre::bail;

use crate::group_store;
use crate::ContextManager;

impl Handler<GetContextAllowlistRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetContextAllowlistRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetContextAllowlistRequest {
            group_id,
            context_id,
        }: GetContextAllowlistRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| -> eyre::Result<Vec<PublicKey>> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group '{group_id:?}' not found");
            }

            group_store::list_context_allowlist(&self.datastore, &group_id, &context_id)
        })();

        ActorResponse::reply(result)
    }
}
