use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_config::types::{GroupRevealPayloadData, SignedGroupRevealPayload, SignerId};
use calimero_context_primitives::group::{JoinGroupRequest, JoinGroupResponse};
use calimero_context_primitives::local_governance::GroupOp;
use calimero_node_primitives::sync::GroupMutationKind;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use eyre::bail;
use sha2::{Digest, Sha256};
use tracing::info;
use tracing::warn;

use crate::config::GroupGovernanceMode;
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
                )))
            }
        };

        let node_sk = node_identity.map(|(_, sk)| sk);
        let signing_key = node_sk;
        let group_governance = self.group_governance;

        let inv = &invitation.invitation;
        let group_id = inv.group_id;
        let protocol = inv.protocol.clone();
        let network_id = inv.network.clone();
        let contract_id = inv.contract_id.clone();
        let expiration_block_height = inv.expiration_height;

        let inviter_identity = PublicKey::from(inv.inviter_identity.to_bytes());

        let needs_chain_sync = group_store::load_group_meta(&self.datastore, &group_id)
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

        let group_client_result = match group_governance {
            GroupGovernanceMode::External => {
                effective_signing_key.map(|sk| self.group_client(group_id, sk))
            }
            GroupGovernanceMode::Local => None,
        };

        let datastore = self.datastore.clone();
        let context_client = self.context_client.clone();
        let node_client = self.node_client.clone();
        let invitation = invitation;

        ActorResponse::r#async(
            async move {
                if needs_chain_sync {
                    if group_governance == GroupGovernanceMode::Local {
                        bail!(
                            "group metadata is missing locally; wait for group state to replicate \
                             before joining (local governance)"
                        );
                    }

                    let (mut meta, _group_info) = group_store::sync_group_state_from_contract(
                        &datastore,
                        &context_client,
                        &group_id,
                        &protocol,
                        &network_id,
                        &contract_id,
                    )
                    .await?;

                    meta.admin_identity = inviter_identity;
                    group_store::save_group_meta(&datastore, &group_id, &meta)?;
                }

                if !group_store::is_group_admin(&datastore, &group_id, &inviter_identity)? {
                    bail!("inviter is no longer an admin of this group");
                }

                if group_store::check_group_membership(&datastore, &group_id, &joiner_identity)? {
                    bail!("identity is already a member of this group");
                }

                match group_governance {
                    GroupGovernanceMode::Local => {
                        let sk = PrivateKey::from(effective_signing_key.ok_or_else(|| {
                            eyre::eyre!(
                                "local group governance requires a signing key for the joiner"
                            )
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

                        let bytes = group_store::sign_apply_local_group_op_borsh(
                            &datastore,
                            &group_id,
                            &sk,
                            GroupOp::JoinWithInvitationClaim {
                                signed_invitation: invitation,
                                invitee_signature_hex,
                            },
                        )?;
                        node_client
                            .publish_signed_group_op(group_id.to_bytes(), bytes)
                            .await?;
                    }
                    GroupGovernanceMode::External => {
                        if let Some(client_result) = group_client_result {
                            let group_client = client_result?;

                            let new_member_signer_id = SignerId::from(*joiner_identity);

                            let reveal_payload_data = GroupRevealPayloadData {
                                signed_open_invitation: invitation.clone(),
                                new_member_identity: new_member_signer_id,
                            };

                            let reveal_data_bytes = borsh::to_vec(&reveal_payload_data)?;
                            let commitment_hash = hex::encode(Sha256::digest(&reveal_data_bytes));

                            group_client
                                .commit_group_invitation(commitment_hash, expiration_block_height)
                                .await?;

                            let effective_key = effective_signing_key.ok_or_else(|| {
                                eyre::eyre!("signing key required for commit/reveal")
                            })?;
                            let joiner_private_key = PrivateKey::from(effective_key);
                            let hash = Sha256::digest(&reveal_data_bytes);
                            let signature = joiner_private_key
                                .sign(&hash)
                                .map_err(|e| eyre::eyre!("signing reveal payload failed: {e}"))?;
                            let invitee_signature = hex::encode(signature.to_bytes());

                            let signed_payload = SignedGroupRevealPayload {
                                data: GroupRevealPayloadData {
                                    signed_open_invitation: invitation,
                                    new_member_identity: new_member_signer_id,
                                },
                                invitee_signature,
                            };

                            group_client.reveal_group_invitation(signed_payload).await?;
                        }

                        group_store::add_group_member(
                            &datastore,
                            &group_id,
                            &joiner_identity,
                            GroupMemberRole::Member,
                        )?;
                    }
                }

                if let Some(ref alias_str) = group_alias {
                    group_store::set_group_alias(&datastore, &group_id, alias_str)?;
                }

                let _ = node_client.subscribe_group(group_id.to_bytes()).await;

                if group_governance == GroupGovernanceMode::External {
                    let _ = node_client
                        .broadcast_group_mutation(
                            group_id.to_bytes(),
                            GroupMutationKind::MembersAdded,
                        )
                        .await;

                    if let Err(err) = group_store::sync_group_state_from_contract(
                        &datastore,
                        &context_client,
                        &group_id,
                        &protocol,
                        &network_id,
                        &contract_id,
                    )
                    .await
                    {
                        warn!(?err, "Failed to sync group state from contract after join");
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
