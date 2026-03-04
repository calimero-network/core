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
