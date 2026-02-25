use actix::{ActorResponse, Handler, Message};
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
        }: DeleteGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
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

            // Remove all members first
            let members =
                group_store::list_group_members(&self.datastore, &group_id, 0, usize::MAX)?;
            for (identity, _role) in &members {
                group_store::remove_group_member(&self.datastore, &group_id, identity)?;
            }

            // Clean up any in-progress or completed upgrade record so crash
            // recovery does not find orphaned entries for deleted groups.
            group_store::delete_group_upgrade(&self.datastore, &group_id)?;

            group_store::delete_group_meta(&self.datastore, &group_id)?;

            info!(?group_id, %requester, "group deleted");

            Ok(DeleteGroupResponse { deleted: true })
        })();

        ActorResponse::reply(result)
    }
}
