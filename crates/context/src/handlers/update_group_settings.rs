use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::UpdateGroupSettingsRequest;
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
        let result = (|| {
            let Some(mut meta) = group_store::load_group_meta(&self.datastore, &group_id)? else {
                bail!("group '{group_id:?}' not found");
            };

            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;

            meta.upgrade_policy = upgrade_policy;
            group_store::save_group_meta(&self.datastore, &group_id, &meta)?;

            Ok(())
        })();

        ActorResponse::reply(result)
    }
}
