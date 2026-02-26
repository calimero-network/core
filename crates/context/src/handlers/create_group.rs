use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::{CreateGroupRequest, CreateGroupResponse};
use calimero_primitives::context::GroupMemberRole;
use calimero_store::key::GroupMetaValue;
use eyre::bail;
use rand::Rng;
use tracing::info;

use crate::group_store;
use crate::ContextManager;

impl Handler<CreateGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <CreateGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        CreateGroupRequest {
            group_id,
            app_key,
            application_id,
            upgrade_policy,
            admin_identity,
        }: CreateGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let group_id = group_id.unwrap_or_else(|| {
                let bytes: [u8; 32] = rand::thread_rng().gen();
                bytes.into()
            });

            if group_store::load_group_meta(&self.datastore, &group_id)?.is_some() {
                bail!("group '{group_id:?}' already exists");
            }

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);

            let meta = GroupMetaValue {
                app_key: app_key.to_bytes(),
                target_application_id: application_id,
                upgrade_policy,
                created_at: now,
                admin_identity: admin_identity.into(),
                migration: None,
            };

            group_store::save_group_meta(&self.datastore, &group_id, &meta)?;
            group_store::add_group_member(
                &self.datastore,
                &group_id,
                &admin_identity,
                GroupMemberRole::Admin,
            )?;

            info!(?group_id, %admin_identity, "group created");

            Ok(CreateGroupResponse { group_id })
        })();

        ActorResponse::reply(result)
    }
}
