use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::ListGroupContextsRequest;
use eyre::bail;

use crate::{group_store, ContextManager};

impl Handler<ListGroupContextsRequest> for ContextManager {
    type Result = ActorResponse<Self, <ListGroupContextsRequest as Message>::Result>;

    fn handle(
        &mut self,
        ListGroupContextsRequest {
            group_id,
            offset,
            limit,
        }: ListGroupContextsRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let Some((node_identity, _)) = self.node_group_identity() else {
                bail!("node has no group identity configured");
            };
            if !group_store::check_group_membership(&self.datastore, &group_id, &node_identity)? {
                bail!("node is not a member of group '{group_id:?}'");
            }
            group_store::enumerate_group_contexts(&self.datastore, &group_id, offset, limit)
        })();

        ActorResponse::reply(result)
    }
}
