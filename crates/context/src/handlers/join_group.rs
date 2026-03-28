use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_config::types::{GroupRevealPayloadData, SignerId};
use calimero_context_config::MemberCapabilities;
use calimero_context_primitives::group::{JoinGroupRequest, JoinGroupResponse};
use calimero_context_primitives::local_governance::GroupOp;
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
                    bail!(
                        "group metadata is missing locally; wait for group state to replicate \
                         before joining (local governance)"
                    );
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

                let _ = node_client.subscribe_group(group_id.to_bytes()).await;

                // Auto-subscribe to all visible contexts if auto_join is set.
                if let Some(meta) = group_store::load_group_meta(&datastore, &group_id)? {
                    if meta.auto_join {
                        let contexts = group_store::enumerate_group_contexts(
                            &datastore,
                            &group_id,
                            0,
                            usize::MAX,
                        )?;
                        for context_id in &contexts {
                            // Write ContextIdentity from group keys so sync
                            // key-share finds the identity for this context.
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

                            if let Err(e) = node_client.subscribe(context_id).await {
                                warn!(
                                    ?group_id,
                                    %context_id,
                                    ?e,
                                    "failed to auto-subscribe to context"
                                );
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
