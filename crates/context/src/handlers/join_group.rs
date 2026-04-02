use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::{JoinGroupRequest, JoinGroupResponse};
use calimero_context_primitives::local_governance::{NamespaceOp, RootOp};
use calimero_primitives::context::{ContextConfigParams, GroupMemberRole};
use calimero_primitives::identity::PrivateKey;
use calimero_store::key;
use eyre::bail;
use tracing::{info, warn};

use crate::{group_store, ContextManager};

impl Handler<JoinGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <JoinGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinGroupRequest {
            invitation,
            group_alias,
        }: JoinGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let group_id = invitation.invitation.group_id;
        let invited_role = invitation.invitation.invited_role;
        let expiration = invitation.invitation.expiration_timestamp;

        if expiration != 0 {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if now > expiration {
                return ActorResponse::reply(Err(eyre::eyre!("invitation expired")));
            }
        }

        let (ns_id, joiner_identity, sk_bytes, _sender) =
            match self.get_or_create_namespace_identity(&group_id) {
                Ok(result) => result,
                Err(err) => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "failed to resolve namespace identity for join: {err}"
                    )));
                }
            };

        let namespace_id = ns_id.to_bytes();
        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let context_client = self.context_client.clone();

        ActorResponse::r#async(
            async move {
                let sk = PrivateKey::from(sk_bytes);
                let role = match invited_role {
                    0 => GroupMemberRole::Admin,
                    2 => GroupMemberRole::ReadOnly,
                    _ => GroupMemberRole::Member,
                };

                // -------------------------------------------------------
                // Phase 1: Set up local state BEFORE syncing so that
                // encrypted namespace ops can be decrypted on arrival.
                // -------------------------------------------------------

                // Store signing key.
                let _ = group_store::store_group_signing_key(
                    &datastore,
                    &group_id,
                    &joiner_identity,
                    &sk_bytes,
                );

                // Store group metadata stub (needed for auto_join flag).
                if group_store::load_group_meta(&datastore, &group_id)?.is_none() {
                    let meta = calimero_store::key::GroupMetaValue {
                        admin_identity: calimero_primitives::identity::PublicKey::from(
                            invitation.invitation.inviter_identity.to_bytes(),
                        ),
                        target_application_id:
                            calimero_primitives::application::ApplicationId::from([0u8; 32]),
                        app_key: [0u8; 32],
                        upgrade_policy: calimero_primitives::context::UpgradePolicy::default(),
                        migration: None,
                        created_at: 0,
                        auto_join: true,
                    };
                    group_store::save_group_meta(&datastore, &group_id, &meta)?;
                }

                // Store membership with sender_key so the decrypt path in
                // apply_signed_namespace_op can find it during sync.
                let sender_key = PrivateKey::random(&mut rand::thread_rng());
                group_store::add_group_member_with_keys(
                    &datastore,
                    &group_id,
                    &joiner_identity,
                    role,
                    Some(*sk),
                    Some(*sender_key),
                )?;

                // -------------------------------------------------------
                // Phase 2: Subscribe + sync namespace governance ops.
                // -------------------------------------------------------

                let _ = node_client.subscribe_namespace(namespace_id).await;

                // Wait for gossipsub mesh to form with existing peers.
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;

                // Pull all namespace governance ops from a peer. Because
                // we already have the sender_key, encrypted ContextRegistered
                // ops will be decrypted and GroupContextIndex written.
                if let Err(e) = node_client.sync_namespace(namespace_id).await {
                    warn!(?e, "namespace sync request failed");
                }
                // Wait for async sync to complete.
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;

                // -------------------------------------------------------
                // Phase 3: Publish MemberJoined + auto-join contexts.
                // -------------------------------------------------------

                let member_joined_op = NamespaceOp::Root(RootOp::MemberJoined {
                    member: joiner_identity,
                    signed_invitation: invitation.clone(),
                });

                group_store::sign_and_publish_namespace_op(
                    &datastore,
                    &node_client,
                    namespace_id,
                    &sk,
                    member_joined_op,
                )
                .await?;

                if let Some(ref alias_str) = group_alias {
                    group_store::set_group_alias(&datastore, &group_id, alias_str)?;
                }

                // Auto-subscribe to visible contexts.
                if let Some(meta) = group_store::load_group_meta(&datastore, &group_id)? {
                    if meta.auto_join {
                        let contexts = group_store::enumerate_group_contexts(
                            &datastore,
                            &group_id,
                            0,
                            usize::MAX,
                        )?;
                        for context_id in &contexts {
                            let mut handle = datastore.handle();
                            let ci_key = key::ContextIdentity::new(*context_id, joiner_identity);
                            if !handle.has(&ci_key)? {
                                handle.put(
                                    &ci_key,
                                    &calimero_store::types::ContextIdentity {
                                        private_key: Some(*sk),
                                        sender_key: Some(*sender_key),
                                    },
                                )?;
                            }
                            drop(handle);

                            let config = if !context_client.has_context(context_id)? {
                                let app_id = group_store::load_group_meta(&datastore, &group_id)?
                                    .map(|m| m.target_application_id);
                                Some(ContextConfigParams {
                                    application_id: app_id,
                                    application_revision: 0,
                                    members_revision: 0,
                                })
                            } else {
                                None
                            };

                            if let Err(e) = context_client
                                .sync_context_config(*context_id, config)
                                .await
                            {
                                warn!(%context_id, ?e, "failed to sync context config");
                            }
                            if let Err(e) = node_client.subscribe(context_id).await {
                                warn!(%context_id, ?e, "failed to subscribe to context");
                            }
                            if let Err(e) = node_client.sync(Some(context_id), None).await {
                                warn!(%context_id, ?e, "failed to trigger context sync");
                            }
                        }
                    }
                }

                // Trigger a global sync for any remaining contexts.
                if let Err(e) = node_client.sync(None, None).await {
                    warn!(?e, "failed to trigger global sync after join");
                }

                info!(
                    ?group_id,
                    namespace_id = %hex::encode(namespace_id),
                    %joiner_identity,
                    "member joined group via namespace MemberJoined"
                );

                Ok(JoinGroupResponse {
                    group_id,
                    member_identity: joiner_identity,
                    governance_op_bytes: vec![],
                })
            }
            .into_actor(self),
        )
    }
}
