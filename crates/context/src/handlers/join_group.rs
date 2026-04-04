use std::sync::Arc;
use std::time::Duration;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{JoinGroupRequest, JoinGroupResponse};
use calimero_context_client::local_governance::{NamespaceOp, RootOp};
use calimero_primitives::context::{ContextConfigParams, GroupMemberRole};
use calimero_primitives::identity::PrivateKey;
use calimero_store::key;
use tokio::sync::Notify;
use tracing::{info, warn};

use crate::{group_store, ContextManager};

const JOIN_OVERALL_TIMEOUT: Duration = Duration::from_secs(30);
const MESH_FORMATION_GRACE: Duration = Duration::from_secs(2);

async fn await_condition<F>(
    notify: &Arc<Notify>,
    deadline: tokio::time::Instant,
    mut check: F,
) -> eyre::Result<bool>
where
    F: FnMut() -> eyre::Result<bool>,
{
    loop {
        if check()? {
            return Ok(true);
        }
        if tokio::time::timeout_at(deadline, notify.notified())
            .await
            .is_err()
        {
            return check();
        }
    }
}

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
        let notify = self.ns_gov_notify.clone();

        ActorResponse::r#async(
            async move {
                let sk = PrivateKey::from(sk_bytes);
                let role = match invited_role {
                    0 => GroupMemberRole::Admin,
                    2 => GroupMemberRole::ReadOnly,
                    _ => GroupMemberRole::Member,
                };

                let deadline = tokio::time::Instant::now() + JOIN_OVERALL_TIMEOUT;

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

                group_store::add_group_member(&datastore, &group_id, &joiner_identity, role)?;

                // -------------------------------------------------------
                // Phase 2: Subscribe + publish MemberJoined, then wait
                //          for the group key to arrive via gossipsub.
                // -------------------------------------------------------

                let _ = node_client.subscribe_namespace(namespace_id).await;

                // Gossipsub mesh formation requires a heartbeat round
                // (~1s default). Wait briefly so that mesh peers are
                // available for both publishing and backfill requests.
                tokio::time::sleep(MESH_FORMATION_GRACE).await;

                // Backfill: ops published before we subscribed won't arrive
                // via gossipsub. Request them once from a mesh peer.
                if let Err(e) = node_client.sync_namespace(namespace_id).await {
                    warn!(?e, "initial namespace backfill request failed");
                }

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

                {
                    let ds = &datastore;
                    let gid = &group_id;
                    let got_key = await_condition(&notify, deadline, || {
                        Ok(group_store::load_current_group_key(ds, gid)?.is_some())
                    })
                    .await?;

                    if !got_key {
                        warn!("group key did not arrive within timeout");
                    } else {
                        info!("received group key via gossipsub");
                    }
                }

                // -------------------------------------------------------
                // Phase 3: Wait for ContextRegistered ops to populate the
                //          group context index, then auto-join.
                // -------------------------------------------------------

                if let Some(ref alias_str) = group_alias {
                    group_store::set_group_alias(&datastore, &group_id, alias_str)?;
                }

                if let Some(meta) = group_store::load_group_meta(&datastore, &group_id)? {
                    if meta.auto_join {
                        // After receiving the key, the encrypted ContextRegistered
                        // ops may not have been backfilled yet. Trigger another
                        // backfill so they arrive and get retried with the key.
                        if let Err(e) = node_client.sync_namespace(namespace_id).await {
                            warn!(?e, "post-key namespace backfill request failed");
                        }

                        let ds = &datastore;
                        let gid = &group_id;
                        let _ = await_condition(&notify, deadline, || {
                            Ok(!group_store::enumerate_group_contexts(ds, gid, 0, 1)?.is_empty())
                        })
                        .await;

                        let contexts = group_store::enumerate_group_contexts(
                            &datastore,
                            &group_id,
                            0,
                            usize::MAX,
                        )?;

                        if contexts.is_empty() {
                            warn!(
                                ?group_id,
                                "no group contexts found within timeout; \
                                 contexts may sync later in the background"
                            );
                        }

                        info!(
                            ?group_id,
                            context_count = contexts.len(),
                            app_id = %meta.target_application_id,
                            "auto-join: enumerated group contexts"
                        );
                        for context_id in &contexts {
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
                                let app_id = group_store::load_group_meta(&datastore, &group_id)?
                                    .map(|m| m.target_application_id)
                                    .filter(|id| *id != zero_app);
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
