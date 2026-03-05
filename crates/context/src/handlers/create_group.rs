use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_config::repr::ReprTransmute;
use calimero_context_config::types as config_types;
use calimero_context_config::types::AppKey;
use calimero_context_primitives::group::{CreateGroupRequest, CreateGroupResponse};
use calimero_primitives::context::GroupMemberRole;
use calimero_store::key::GroupMetaValue;
use calimero_store::Store;
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
            signing_key,
        }: CreateGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_near_identity();

        // Resolve admin_identity: use provided value or fall back to node NEAR identity
        let admin_identity = match admin_identity {
            Some(pk) => pk,
            None => match node_identity {
                Some((pk, _)) => pk,
                None => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "admin_identity not provided and node has no configured NEAR identity"
                    )))
                }
            },
        };

        // Resolve app_key: use provided value or generate random
        let app_key = app_key.unwrap_or_else(|| {
            let bytes: [u8; 32] = rand::thread_rng().gen();
            AppKey::from(bytes)
        });

        // Resolve signing_key: prefer explicit, then node identity key, then stored key
        let node_sk = node_identity.map(|(_, sk)| sk);
        let signing_key = signing_key.or(node_sk);

        // Sync validation
        let group_id = group_id.unwrap_or_else(|| {
            let bytes: [u8; 32] = rand::thread_rng().gen();
            bytes.into()
        });

        if let Ok(Some(_)) = group_store::load_group_meta(&self.datastore, &group_id) {
            return ActorResponse::reply(Err(eyre::eyre!("group '{group_id:?}' already exists")));
        }

        // Load application meta to build contract Application type
        let app_meta = match load_app_meta(&self.datastore, &application_id) {
            Ok(meta) => meta,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let datastore = self.datastore.clone();

        // Auto-store signing key for future use (group is about to be created with
        // admin_identity as the first admin, so store it keyed to that identity)
        if let Some(ref sk) = signing_key {
            let _ = group_store::store_group_signing_key(
                &self.datastore,
                &group_id,
                &admin_identity,
                sk,
            );
        }

        // Build group_client if signing_key available, falling back to stored key
        let effective_signing_key = signing_key.or_else(|| {
            group_store::get_group_signing_key(&self.datastore, &group_id, &admin_identity)
                .ok()
                .flatten()
        });
        let group_client_result = effective_signing_key.map(|sk| self.group_client(group_id, sk));

        ActorResponse::r#async(
            async move {
                // Call contract if signing_key was provided
                if let Some(client_result) = group_client_result {
                    let mut group_client = client_result?;

                    let contract_app = build_contract_application(&application_id, &app_meta)?;

                    group_client.create_group(app_key, contract_app).await?;
                }

                // Local cache write
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

                group_store::save_group_meta(&datastore, &group_id, &meta)?;
                group_store::add_group_member(
                    &datastore,
                    &group_id,
                    &admin_identity,
                    GroupMemberRole::Admin,
                )?;

                info!(?group_id, %admin_identity, "group created");

                Ok(CreateGroupResponse { group_id })
            }
            .into_actor(self),
        )
    }
}

fn load_app_meta(
    datastore: &Store,
    application_id: &calimero_primitives::application::ApplicationId,
) -> eyre::Result<calimero_store::types::ApplicationMeta> {
    let handle = datastore.handle();
    let key = calimero_store::key::ApplicationMeta::new(*application_id);
    handle
        .get(&key)?
        .ok_or_else(|| eyre::eyre!("application '{application_id}' not found"))
}

pub(crate) fn build_contract_application(
    application_id: &calimero_primitives::application::ApplicationId,
    app_meta: &calimero_store::types::ApplicationMeta,
) -> eyre::Result<config_types::Application<'static>> {
    Ok(config_types::Application::new(
        application_id.rt()?,
        app_meta.bytecode.blob_id().rt()?,
        app_meta.size,
        config_types::ApplicationSource(app_meta.source.to_string().into()),
        config_types::ApplicationMetadata(calimero_context_config::repr::Repr::new(
            app_meta.metadata.to_vec().into(),
        )),
    ))
}
