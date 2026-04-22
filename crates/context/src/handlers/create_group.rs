use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{CreateGroupRequest, CreateGroupResponse};
use calimero_context_client::local_governance::{NamespaceOp, RootOp};
use calimero_context_config::types::AppKey;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_store::key::GroupMetaValue;
use calimero_store::Store;
use rand::Rng;
use tracing::{info, warn};

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
            alias,
            parent_group_id,
        }: CreateGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Resolve app_key: use provided value or generate random
        let app_key = app_key.unwrap_or_else(|| {
            let bytes: [u8; 32] = rand::thread_rng().gen();
            AppKey::from(bytes)
        });

        let group_id = group_id.unwrap_or_else(|| {
            let bytes: [u8; 32] = rand::thread_rng().gen();
            bytes.into()
        });

        if let Ok(Some(_)) = group_store::load_group_meta(&self.datastore, &group_id) {
            return ActorResponse::reply(Err(eyre::eyre!("group '{group_id:?}' already exists")));
        }

        let namespace_anchor_group_id = parent_group_id.as_ref().unwrap_or(&group_id);
        let (namespace_id, admin_identity, sk_bytes, _sender) =
            match self.get_or_create_namespace_identity(namespace_anchor_group_id) {
                Ok(result) => result,
                Err(err) => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "failed to resolve namespace identity: {err}"
                    )))
                }
            };

        let signing_key = Some(sk_bytes);

        // Subgroups inherit target_application_id from the parent (namespace root owns the app).
        let effective_application_id = if let Some(ref parent_id) = parent_group_id {
            let parent_meta = match group_store::load_group_meta(&self.datastore, parent_id) {
                Ok(Some(m)) => m,
                _ => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "parent group '{parent_id:?}' not found"
                    )));
                }
            };
            if let Err(err) =
                group_store::require_group_admin(&self.datastore, parent_id, &admin_identity)
            {
                return ActorResponse::reply(Err(err));
            }
            parent_meta.target_application_id
        } else {
            application_id
        };

        if let Err(err) = load_app_meta(&self.datastore, &effective_application_id) {
            return ActorResponse::reply(Err(err));
        }

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();

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

        ActorResponse::r#async(
            async move {
                // Local cache write
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                let meta = GroupMetaValue {
                    app_key: app_key.to_bytes(),
                    target_application_id: effective_application_id,
                    upgrade_policy,
                    created_at: now,
                    admin_identity: admin_identity.into(),
                    migration: None,
                    auto_join: true,
                };

                group_store::save_group_meta(&datastore, &group_id, &meta)?;
                group_store::add_group_member(
                    &datastore,
                    &group_id,
                    &admin_identity,
                    GroupMemberRole::Admin,
                )?;

                // Set default capabilities so new members can join open contexts
                group_store::set_default_capabilities(
                    &datastore,
                    &group_id,
                    calimero_context_config::MemberCapabilities::CAN_JOIN_OPEN_CONTEXTS,
                )?;

                // Generate and store the group encryption key.
                let group_key: [u8; 32] = rand::thread_rng().gen();
                let key_id = group_store::store_group_key(&datastore, &group_id, &group_key)?;
                tracing::debug!(
                    ?group_id,
                    key_id = %hex::encode(key_id),
                    "stored initial group key"
                );

                if let Some(ref alias_str) = alias {
                    group_store::set_group_alias(&datastore, &group_id, alias_str)?;
                }

                // In the namespace model, group hierarchy is tracked in the
                // namespace DAG (RootOp::GroupCreated), not via parent refs.
                if let Err(err) = node_client
                    .subscribe_namespace(namespace_id.to_bytes())
                    .await
                {
                    warn!(
                        ?err,
                        namespace_id=%hex::encode(namespace_id.to_bytes()),
                        "failed to subscribe to namespace before publishing governance ops"
                    );
                }

                let signer_sk = PrivateKey::from(sk_bytes);
                // Strict-tree refactor: GroupCreated is now an atomic
                // create+nest op. It ONLY applies to subgroups — the namespace
                // root itself has no parent by definition. For root creation
                // (parent_group_id is None), the group's existence is recorded
                // by the pre-populated GroupMeta and the namespace identity in
                // the store; we skip the op entirely. Peers learn of a
                // namespace only when they're invited (MemberJoined), so there's
                // no replication gap. See spec
                // docs/superpowers/specs/2026-04-22-strict-group-tree-and-cascade-delete.md
                if let Some(parent_id) = parent_group_id {
                    let create_op = NamespaceOp::Root(RootOp::GroupCreated {
                        group_id: group_id.to_bytes(),
                        parent_id: parent_id.to_bytes(),
                    });
                    if let Err(e) = group_store::sign_apply_and_publish_namespace_op(
                        &datastore,
                        &node_client,
                        namespace_id.to_bytes(),
                        &signer_sk,
                        create_op,
                    )
                    .await
                    {
                        tracing::warn!(?e, "failed to publish GroupCreated on namespace DAG");
                    }
                }

                info!(?group_id, ?parent_group_id, %admin_identity, "group created");

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
