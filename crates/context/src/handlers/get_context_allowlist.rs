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
            let Some((node_identity, _)) = self.node_group_identity() else {
                bail!("node has no group identity configured");
            };
            if !group_store::check_group_membership(&self.datastore, &group_id, &node_identity)? {
                bail!("node is not a member of group '{group_id:?}'");
            }

            group_store::list_context_allowlist(&self.datastore, &group_id, &context_id)
        })();

        ActorResponse::reply(result)
    }
}
