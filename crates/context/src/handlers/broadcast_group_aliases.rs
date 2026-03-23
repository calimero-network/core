use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::BroadcastGroupAliasesRequest;
use calimero_node_primitives::sync::GroupMutationKind;

use crate::{group_store, ContextManager};

impl Handler<BroadcastGroupAliasesRequest> for ContextManager {
    type Result = ActorResponse<Self, <BroadcastGroupAliasesRequest as Message>::Result>;

    fn handle(
        &mut self,
        BroadcastGroupAliasesRequest { group_id }: BroadcastGroupAliasesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let entries = match group_store::enumerate_group_contexts_with_aliases(
            &self.datastore,
            &group_id,
            0,
            usize::MAX,
        ) {
            Ok(e) => e,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let node_client = self.node_client.clone();
        let group_id_bytes = group_id.to_bytes();

        ActorResponse::r#async(
            async move {
                for (context_id, alias) in entries {
                    let Some(alias) = alias else { continue };
                    let _ = node_client
                        .broadcast_group_mutation(
                            group_id_bytes,
                            GroupMutationKind::ContextAliasSet {
                                context_id: *context_id,
                                alias,
                            },
                        )
                        .await;
                }
                Ok(())
            }
            .into_actor(self),
        )
    }
}
