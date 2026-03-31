use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_config::types::{GroupRevealPayloadData, SignerId};
use calimero_context_config::MemberCapabilities;
use calimero_context_primitives::group::{JoinGroupRequest, JoinGroupResponse};
use calimero_context_primitives::local_governance::{GroupOp, SignedGroupOp};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key;
use eyre::bail;
use sha2::{Digest, Sha256};
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
        let node_identity = self.node_group_identity();

        let joiner_identity = match node_identity {
            Some((pk, _)) => pk,
            None => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "joiner_identity not provided and node has no configured group identity"
                )));
            }
        };

        let node_sk = node_identity.map(|(_, sk)| sk);
        let signing_key = node_sk;

        let inv = &invitation.invitation;
        let group_id = inv.group_id;

        let inviter_identity = PublicKey::from(inv.inviter_identity.to_bytes());

        let group_not_found_locally = group_store::load_group_meta(&self.datastore, &group_id)
            .map(|opt| opt.is_none())
            .unwrap_or(true);

        if let Some(ref sk) = signing_key {
            let _ = group_store::store_group_signing_key(
                &self.datastore,
                &group_id,
                &joiner_identity,
                sk,
            );
        }

        let effective_signing_key = signing_key.or_else(|| {
            group_store::get_group_signing_key(&self.datastore, &group_id, &joiner_identity)
                .ok()
                .flatten()
        });

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let invitation = invitation;

        ActorResponse::r#async(
            async move {
                if group_not_found_locally {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    group_store::save_group_meta(
                        &datastore,
                        &group_id,
                        &key::GroupMetaValue {
                            app_key: [0u8; 32],
                            target_application_id:
                                calimero_primitives::application::ApplicationId::from([0u8; 32]),
                            upgrade_policy:
                                calimero_primitives::context::UpgradePolicy::default(),
                            created_at: now,
                            admin_identity: inviter_identity,
                            migration: None,
                            auto_join: true,
                        },
                    )?;
                    if !group_store::check_group_membership(
                        &datastore,
                        &group_id,
                        &inviter_identity,
                    )? {
                        group_store::add_group_member(
                            &datastore,
                            &group_id,
                            &inviter_identity,
                            GroupMemberRole::Admin,
                        )?;
                    }
                    let _ = node_client.subscribe_group(group_id.to_bytes()).await;
                }

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

                let governance_op = GroupOp::JoinWithInvitationClaim {
                    signed_invitation: invitation,
                    invitee_signature_hex,
                };

                if group_not_found_locally {
                    // Bootstrapped stub metadata — use zero state hash so the
                    // admin's node (which has the real metadata) will accept
                    // the op without a state-hash mismatch.
                    let nonce = group_store::get_local_gov_nonce(
                        &datastore,
                        &group_id,
                        &joiner_identity,
                    )?
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| eyre::eyre!("nonce overflow"))?;
                    let parent_hashes = group_store::get_op_head(&datastore, &group_id)?
                        .map(|h| h.dag_heads.clone())
                        .unwrap_or_default();

                    let signed_op = SignedGroupOp::sign(
                        &sk,
                        group_id.to_bytes(),
                        parent_hashes,
                        [0u8; 32],
                        nonce,
                        governance_op,
                    )
                    .map_err(|e| eyre::eyre!("signing governance op failed: {e}"))?;

                    if let Err(e) =
                        group_store::apply_local_signed_group_op(&datastore, &signed_op)
                    {
                        warn!(%joiner_identity, %e, "failed to apply JoinWithInvitationClaim locally (bootstrapped)");
                    }
                    let delta_id = signed_op
                        .content_hash()
                        .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
                    let parent_ids = signed_op.parent_op_hashes.clone();
                    let bytes = borsh::to_vec(&signed_op)?;
                    node_client
                        .publish_signed_group_op(
                            group_id.to_bytes(),
                            delta_id,
                            parent_ids,
                            bytes,
                        )
                        .await?;
                } else {
                    group_store::sign_apply_and_publish(
                        &datastore,
                        &node_client,
                        &group_id,
                        &sk,
                        governance_op,
                    )
                    .await?;
                }

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

                let _ = node_client.subscribe_group(group_id.to_bytes()).await;

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

                                if let Err(e) = node_client.subscribe(context_id).await {
                                    warn!(
                                        ?gid,
                                        %context_id,
                                        ?e,
                                        "failed to auto-subscribe to context"
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
