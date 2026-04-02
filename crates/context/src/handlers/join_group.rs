use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_config::types::{GroupRevealPayloadData, SignerId};
use calimero_context_config::MemberCapabilities;
use calimero_context_primitives::group::{JoinGroupRequest, JoinGroupResponse};
use calimero_context_primitives::local_governance::GroupOp;
use calimero_primitives::context::{ContextConfigParams, GroupMemberRole};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key;
use eyre::bail;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::{group_store, ContextManager};

/// Maximum number of attempts to poll for group metadata arrival.
const META_POLL_MAX_ATTEMPTS: u32 = 10;
/// Interval between metadata polling attempts.
const META_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

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
        let inv = &invitation.invitation;
        let group_id = inv.group_id;

        let (_, joiner_identity, sk_bytes, _sender) =
            match self.get_or_create_namespace_identity(&group_id) {
                Ok(result) => result,
                Err(err) => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "failed to resolve namespace identity for join: {err}"
                    )));
                }
            };

        let signing_key = Some(sk_bytes);

        let inviter_identity = PublicKey::from(inv.inviter_identity.to_bytes());

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let context_client = self.context_client.clone();
        let invitation = invitation;

        ActorResponse::r#async(
            async move {
                // Subscribe to the group topic first so the existing member
                // broadcasts group metadata to us via gossipsub.
                let _ = node_client.subscribe_group(group_id.to_bytes()).await;

                // Poll for group metadata to arrive from the peer broadcast.
                let mut meta_found = false;
                for _ in 0..META_POLL_MAX_ATTEMPTS {
                    if group_store::load_group_meta(&datastore, &group_id)?.is_some() {
                        meta_found = true;
                        break;
                    }
                    tokio::time::sleep(META_POLL_INTERVAL).await;
                }
                if !meta_found {
                    bail!(
                        "group metadata is missing locally; timed out waiting for group state \
                         to replicate before joining (local governance)"
                    );
                }

                if let Some(ref sk) = signing_key {
                    let _ = group_store::store_group_signing_key(
                        &datastore,
                        &group_id,
                        &joiner_identity,
                        sk,
                    );
                }

                let effective_signing_key = signing_key.or_else(|| {
                    group_store::get_group_signing_key(&datastore, &group_id, &joiner_identity)
                        .ok()
                        .flatten()
                });

                if !group_store::is_group_admin_or_has_capability(
                    &datastore,
                    &group_id,
                    &inviter_identity,
                    MemberCapabilities::CAN_INVITE_MEMBERS,
                )? {
                    bail!("inviter lacks permission (not admin and missing CAN_INVITE_MEMBERS)");
                }

                if group_store::check_group_membership(&datastore, &group_id, &joiner_identity)? {
                    bail!("identity is already a member of this group");
                }

                let sk = PrivateKey::from(effective_signing_key.ok_or_else(|| {
                    eyre::eyre!("local group governance requires a signing key for the joiner")
                })?);
                let reveal_payload_data = GroupRevealPayloadData {
                    signed_open_invitation: invitation.clone(),
                    new_member_identity: SignerId::from(*joiner_identity.digest()),
                };
                let reveal_data_bytes = borsh::to_vec(&reveal_payload_data)?;
                let hash = Sha256::digest(&reveal_data_bytes);
                let signature = sk
                    .sign(&hash)
                    .map_err(|e| eyre::eyre!("signing reveal payload failed: {e}"))?;
                let invitee_signature_hex = hex::encode(signature.to_bytes());

                group_store::sign_apply_and_publish(
                    &datastore,
                    &node_client,
                    &group_id,
                    &sk,
                    GroupOp::JoinWithInvitationClaim {
                        signed_invitation: invitation,
                        invitee_signature_hex,
                    },
                )
                .await?;

                // Upgrade the GroupMember entry with local private + sender keys
                // so the sync key-share can use them for all contexts in the group.
                let sender_key = PrivateKey::random(&mut rand::thread_rng());
                group_store::add_group_member_with_keys(
                    &datastore,
                    &group_id,
                    &joiner_identity,
                    GroupMemberRole::Member,
                    Some(*sk),
                    Some(*sender_key),
                )?;

                if let Some(ref alias_str) = group_alias {
                    group_store::set_group_alias(&datastore, &group_id, alias_str)?;
                }

                // Auto-subscribe to all visible contexts if auto_join is set,
                // including contexts in child subgroups (membership inherits down).
                if let Some(meta) = group_store::load_group_meta(&datastore, &group_id)? {
                    if meta.auto_join {
                        let mut groups_to_visit = vec![group_id];
                        let mut depth = 0u8;
                        while let Some(gid) = groups_to_visit.pop() {
                            let contexts = group_store::enumerate_group_contexts(
                                &datastore,
                                &gid,
                                0,
                                usize::MAX,
                            )?;
                            for context_id in &contexts {
                                let mut handle = datastore.handle();
                                let ci_key =
                                    key::ContextIdentity::new(*context_id, joiner_identity);
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
                                    let app_id = group_store::load_group_meta(&datastore, &gid)?
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
                                    warn!(
                                        ?gid,
                                        %context_id,
                                        ?e,
                                        "failed to sync context config during auto-join"
                                    );
                                }

                                if let Err(e) = node_client.subscribe(context_id).await {
                                    warn!(
                                        ?gid,
                                        %context_id,
                                        ?e,
                                        "failed to auto-subscribe to context"
                                    );
                                }
                                if let Err(e) = node_client.sync(Some(context_id), None).await {
                                    warn!(
                                        ?gid,
                                        %context_id,
                                        ?e,
                                        "failed to trigger sync after auto-join"
                                    );
                                }
                            }

                            if depth < 16 {
                                if let Ok(children) =
                                    group_store::enumerate_child_groups(&datastore, &gid)
                                {
                                    for child_id in children {
                                        let _ =
                                            node_client.subscribe_group(child_id.to_bytes()).await;
                                        groups_to_visit.push(child_id);
                                    }
                                }
                                depth += 1;
                            }
                        }
                    }
                }

                info!(
                    ?group_id,
                    %joiner_identity,
                    "new member joined group via invitation"
                );

                Ok(JoinGroupResponse {
                    group_id,
                    member_identity: joiner_identity,
                })
            }
            .into_actor(self),
        )
    }
}
