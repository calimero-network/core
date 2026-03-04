use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::{DeleteGroupRequest, DeleteGroupResponse};
use eyre::bail;
use tracing::info;

use crate::group_store;
use crate::ContextManager;

impl Handler<DeleteGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <DeleteGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        DeleteGroupRequest {
            group_id,
            requester,
            signing_key,
        }: DeleteGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Sync validation
        if let Err(err) = (|| -> eyre::Result<()> {
            let Some(_meta) = group_store::load_group_meta(&self.datastore, &group_id)? else {
                bail!("group '{group_id:?}' not found");
            };
            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;
            let ctx_count = group_store::count_group_contexts(&self.datastore, &group_id)?;
            if ctx_count > 0 {
                bail!(
                    "cannot delete group '{group_id:?}': still has {ctx_count} associated context(s)"
                );
            }
            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        let datastore = self.datastore.clone();
        let group_client_result = signing_key.map(|sk| self.group_client(group_id, sk));

        ActorResponse::r#async(
            async move {
                if let Some(client_result) = group_client_result {
                    let mut group_client = client_result?;
                    group_client.delete_group().await?;
                }

                // Remove all members in bounded batches to cap peak allocation
                loop {
                    let batch = group_store::list_group_members(&datastore, &group_id, 0, 500)?;
                    if batch.is_empty() {
                        break;
                    }
                    for (identity, _role) in &batch {
                        group_store::remove_group_member(&datastore, &group_id, identity)?;
                    }
                }

                // Clean up any in-progress or completed upgrade record so crash
                // recovery does not find orphaned entries for deleted groups.
                group_store::delete_group_upgrade(&datastore, &group_id)?;
                group_store::delete_group_meta(&datastore, &group_id)?;

                info!(?group_id, %requester, "group deleted");

                Ok(DeleteGroupResponse { deleted: true })
            }
            .into_actor(self),
        )
    }
}
