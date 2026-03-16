use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::SetGroupAliasRequest;
use calimero_node_primitives::sync::GroupMutationKind;
use eyre::bail;
use tracing::info;

use crate::{group_store, ContextManager};

impl Handler<SetGroupAliasRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetGroupAliasRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetGroupAliasRequest {
            group_id,
            alias,
            requester,
        }: SetGroupAliasRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_group_identity();

        let requester = match requester {
            Some(pk) => pk,
            None => match node_identity {
                Some((pk, _)) => pk,
                None => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "requester not provided and node has no configured group identity"
                    )))
                }
            },
        };

        if let Err(err) = (|| -> eyre::Result<()> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group '{group_id:?}' not found");
            }

            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;

            group_store::set_group_alias(&self.datastore, &group_id, &alias)?;

            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        let node_client = self.node_client.clone();

        ActorResponse::r#async(
            async move {
                info!(
                    ?group_id,
                    %alias,
                    "group alias set"
                );

                let _ = node_client
                    .broadcast_group_mutation(
                        group_id.to_bytes(),
                        GroupMutationKind::GroupAliasSet { alias },
                    )
                    .await;

                Ok(())
            }
            .into_actor(self),
        )
    }
}
