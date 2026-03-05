use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::UpdateGroupSettingsRequest;
use calimero_node_primitives::sync::GroupMutationKind;
use eyre::bail;

use crate::group_store;
use crate::ContextManager;

impl Handler<UpdateGroupSettingsRequest> for ContextManager {
    type Result = ActorResponse<Self, <UpdateGroupSettingsRequest as Message>::Result>;

    fn handle(
        &mut self,
        UpdateGroupSettingsRequest {
            group_id,
            requester,
            upgrade_policy,
        }: UpdateGroupSettingsRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_near_identity();

        // Resolve requester: use provided value or fall back to node NEAR identity
        let requester = match requester {
            Some(pk) => pk,
            None => match node_identity {
                Some((pk, _)) => pk,
                None => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "requester not provided and node has no configured NEAR identity"
                    )))
                }
            },
        };

        // Auto-store node signing key so it's available for authorization checks
        if let Some((_, node_sk)) = node_identity {
            let _ = group_store::store_group_signing_key(
                &self.datastore,
                &group_id,
                &requester,
                &node_sk,
            );
        }

        if let Err(err) = (|| -> eyre::Result<()> {
            let Some(mut meta) = group_store::load_group_meta(&self.datastore, &group_id)? else {
                bail!("group '{group_id:?}' not found");
            };

            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;
            group_store::require_group_signing_key(&self.datastore, &group_id, &requester)?;

            meta.upgrade_policy = upgrade_policy;
            group_store::save_group_meta(&self.datastore, &group_id, &meta)?;

            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();

        ActorResponse::r#async(
            async move {
                let contexts =
                    group_store::enumerate_group_contexts(&datastore, &group_id, 0, usize::MAX)?;
                let _ = node_client
                    .broadcast_group_mutation(
                        &contexts,
                        group_id.to_bytes(),
                        GroupMutationKind::SettingsUpdated,
                    )
                    .await;
                Ok(())
            }
            .into_actor(self),
        )
    }
}
