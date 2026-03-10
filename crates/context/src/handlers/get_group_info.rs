use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::{GetGroupInfoRequest, GroupInfoResponse};
use eyre::bail;

use crate::group_store;
use crate::ContextManager;

impl Handler<GetGroupInfoRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetGroupInfoRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetGroupInfoRequest { group_id }: GetGroupInfoRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let Some(meta) = group_store::load_group_meta(&self.datastore, &group_id)? else {
                bail!("group '{group_id:?}' not found");
            };

            let member_count = group_store::count_group_members(&self.datastore, &group_id)? as u64;

            let context_count =
                group_store::count_group_contexts(&self.datastore, &group_id)? as u64;

            let active_upgrade =
                group_store::load_group_upgrade(&self.datastore, &group_id)?.map(Into::into);

            let default_capabilities =
                group_store::get_default_capabilities(&self.datastore, &group_id)?.unwrap_or(0);

            let default_visibility =
                match group_store::get_default_visibility(&self.datastore, &group_id)?.unwrap_or(0)
                {
                    1 => "restricted".to_owned(),
                    _ => "open".to_owned(),
                };

            Ok(GroupInfoResponse {
                group_id,
                app_key: meta.app_key.into(),
                target_application_id: meta.target_application_id,
                upgrade_policy: meta.upgrade_policy,
                member_count,
                context_count,
                active_upgrade,
                default_capabilities,
                default_visibility,
            })
        })();

        ActorResponse::reply(result)
    }
}
