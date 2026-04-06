use std::time::Duration;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{JoinGroupRequest, JoinGroupResponse};
use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_primitives::context::{ContextConfigParams, GroupMemberRole};
use calimero_primitives::identity::PrivateKey;
use calimero_store::key;
use tracing::{info, warn};

use crate::{group_store, ContextManager};

const NAMESPACE_MESH_GRACE: Duration = Duration::from_secs(2);

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

        let (ns_id, joiner_identity, sk_bytes, _sender_key_bytes) =
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
                // Phase 1: Set up local state.
                // -------------------------------------------------------

                let _ = group_store::store_group_signing_key(
                    &datastore,
                    &group_id,
                    &joiner_identity,
                    &sk_bytes,
                );

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

                if !group_store::check_group_membership(&datastore, &group_id, &joiner_identity)? {
                    group_store::add_group_member(&datastore, &group_id, &joiner_identity, role)?;
                } else {
                    info!(
                        ?group_id,
                        %joiner_identity,
                        "group member already recorded locally, skipping add_group_member"
                    );
                }

                // -------------------------------------------------------
                // Phase 2: Subscribe to namespace topic, wait for mesh
                //          formation, then get everything we need via a
                //          single direct stream request to a mesh peer.
                // -------------------------------------------------------

                let _ = node_client.subscribe_namespace(namespace_id).await;
                tokio::time::sleep(NAMESPACE_MESH_GRACE).await;

                let invitation_bytes = borsh::to_vec(&invitation)
                    .map_err(|e| eyre::eyre!("failed to serialize invitation: {e}"))?;

                let join_result = node_client
                    .request_namespace_join(namespace_id, invitation_bytes, joiner_identity)
                    .await?;

                // Unwrap and store the group key.
                if !join_result.has_key() {
                    warn!("join response contained no group key");
                } else if group_store::load_current_group_key(&datastore, &group_id)?.is_some() {
                    info!(
                        ?group_id,
                        "group key already present locally, skipping store from join response"
                    );
                } else {
                    let envelope: calimero_context_client::local_governance::KeyEnvelope =
                        borsh::from_slice(&join_result.key_envelope_bytes)
                            .map_err(|e| eyre::eyre!("failed to deserialize key envelope: {e}"))?;

                    let group_key = group_store::unwrap_group_key(&sk, &envelope)?;
                    group_store::store_group_key(&datastore, &group_id, &group_key)?;
                    info!("received group key via direct join response");
                }

                // Apply governance ops so the local DAG is up to date.
                for op_bytes in &join_result.governance_ops {
                    if let Ok(op) = borsh::from_slice::<SignedNamespaceOp>(op_bytes) {
                        if let Err(e) = context_client.apply_signed_namespace_op(op).await {
                            warn!(?e, "failed to apply governance op from join response");
                        }
                    }
                }

                // Pull any governance ops published during (or just before) the
                // join window that weren't in the join response snapshot.
                // Direct stream request — does not depend on gossip delivery.
                if let Err(e) = node_client.sync_namespace(namespace_id).await {
                    warn!(?e, "failed to trigger post-join namespace governance pull (non-fatal)");
                }

                // Publish MemberJoined so other namespace members learn
                // about us (fire-and-forget, the joiner doesn't depend on it).
                let member_joined_op = NamespaceOp::Root(RootOp::MemberJoined {
                    member: joiner_identity,
                    signed_invitation: invitation.clone(),
                });
                if let Err(e) = group_store::sign_and_publish_namespace_op(
                    &datastore,
                    &node_client,
                    namespace_id,
                    &sk,
                    member_joined_op,
                )
                .await
                {
                    warn!(?e, "failed to publish MemberJoined (non-fatal)");
                }

                // -------------------------------------------------------
                // Phase 3: Auto-join contexts from the response.
                // -------------------------------------------------------

                if let Some(ref alias_str) = group_alias {
                    group_store::set_group_alias(&datastore, &group_id, alias_str)?;
                }

                let contexts = &join_result.context_ids;

                if let Some(meta) = group_store::load_group_meta(&datastore, &group_id)? {
                    if meta.auto_join {
                        info!(
                            ?group_id,
                            context_count = contexts.len(),
                            "auto-join: contexts from direct join response"
                        );
                        for context_id in contexts {
                            let mut handle = datastore.handle();
                            let ci_key = key::ContextIdentity::new(*context_id, joiner_identity);
                            if !handle.has(&ci_key)? {
                                handle.put(
                                    &ci_key,
                                    &calimero_store::types::ContextIdentity {
                                        private_key: Some(*sk),
                                        sender_key: None,
                                    },
                                )?;
                            }
                            drop(handle);

                            let config = if !context_client.has_context(context_id)? {
                                let zero_app =
                                    calimero_primitives::application::ApplicationId::from(
                                        [0u8; 32],
                                    );
                                let app_id = join_result.application_id;
                                let resolved = if app_id != zero_app {
                                    Some(app_id)
                                } else {
                                    group_store::load_group_meta(&datastore, &group_id)?
                                        .map(|m| m.target_application_id)
                                        .filter(|id| *id != zero_app)
                                };
                                Some(ContextConfigParams {
                                    application_id: resolved,
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

                if let Err(e) = node_client.sync(None, None).await {
                    warn!(?e, "failed to trigger global sync after join");
                }

                info!(
                    ?group_id,
                    namespace_id = %hex::encode(namespace_id),
                    %joiner_identity,
                    "member joined group via direct request-response"
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
