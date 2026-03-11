use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_config::types::{GroupRevealPayloadData, SignedGroupRevealPayload, SignerId};
use calimero_context_primitives::group::{JoinGroupRequest, JoinGroupResponse};
use calimero_node_primitives::sync::GroupMutationKind;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use eyre::bail;
use sha2::{Digest, Sha256};
use tracing::info;

use crate::{group_store, ContextManager};

impl Handler<JoinGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <JoinGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinGroupRequest { invitation }: JoinGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_group_identity();

        // Resolve joiner_identity from node group identity
        let joiner_identity = match node_identity {
            Some((pk, _)) => pk,
            None => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "joiner_identity not provided and node has no configured group identity"
                )))
            }
        };

        // Resolve signing_key from node identity key
        let node_sk = node_identity.map(|(_, sk)| sk);
        let signing_key = node_sk;

        // Extract fields directly from the SignedGroupOpenInvitation
        let inv = &invitation.invitation;
        let group_id = inv.group_id;
        let protocol = inv.protocol.clone();
        let network_id = inv.network.clone();
        let contract_id = inv.contract_id.clone();
        let expiration_block_height = inv.expiration_height;

        // Derive inviter_identity (PublicKey) for local admin validation
        let inviter_identity = match (|| -> eyre::Result<PublicKey> {
            let bytes: [u8; 32] = borsh::from_slice(
                &borsh::to_vec(&inv.inviter_identity)
                    .map_err(|e| eyre::eyre!("borsh serialize inviter_identity: {e}"))?,
            )?;
            Ok(PublicKey::from(bytes))
        })() {
            Ok(pk) => pk,
            Err(e) => return ActorResponse::reply(Err(e)),
        };

        // Check if we need to bootstrap from chain
        let needs_chain_sync = group_store::load_group_meta(&self.datastore, &group_id)
            .map(|opt| opt.is_none())
            .unwrap_or(true);

        // Auto-store signing key if provided
        if let Some(ref sk) = signing_key {
            let _ = group_store::store_group_signing_key(
                &self.datastore,
                &group_id,
                &joiner_identity,
                sk,
            );
        }

        // Resolve effective signing key (provided, node key, or previously stored)
        let effective_signing_key = signing_key.or_else(|| {
            group_store::get_group_signing_key(&self.datastore, &group_id, &joiner_identity)
                .ok()
                .flatten()
        });

        let group_client_result = effective_signing_key.map(|sk| self.group_client(group_id, sk));

        let datastore = self.datastore.clone();
        let context_client = self.context_client.clone();
        let node_client = self.node_client.clone();

        ActorResponse::r#async(
            async move {
                // Phase 1: Bootstrap from chain if local state is missing
                if needs_chain_sync {
                    let (mut meta, _group_info) = group_store::sync_group_state_from_contract(
                        &datastore,
                        &context_client,
                        &group_id,
                        &protocol,
                        &network_id,
                        &contract_id,
                    )
                    .await?;

                    // Set admin_identity to inviter (who created the invitation)
                    meta.admin_identity = inviter_identity;
                    group_store::save_group_meta(&datastore, &group_id, &meta)?;
                    // sync_group_state_from_contract already reflects on-chain member state;
                    // do NOT re-insert the inviter as Admin here — that would make the
                    // is_group_admin check below trivially pass even for demoted admins.
                }

                // Phase 2: Validate
                if !group_store::is_group_admin(&datastore, &group_id, &inviter_identity)? {
                    bail!("inviter is no longer an admin of this group");
                }

                if group_store::check_group_membership(&datastore, &group_id, &joiner_identity)? {
                    bail!("identity is already a member of this group");
                }

                // Phase 3: Contract commit/reveal + local store
                if let Some(client_result) = group_client_result {
                    let group_client = client_result?;

                    let joiner_bytes: [u8; 32] = *joiner_identity;
                    let new_member_signer_id: SignerId =
                        borsh::from_slice(&borsh::to_vec(&joiner_bytes)?)?;

                    // Build the reveal payload data using the invitation directly
                    let reveal_payload_data = GroupRevealPayloadData {
                        signed_open_invitation: invitation,
                        new_member_identity: new_member_signer_id,
                    };

                    // Compute commitment hash
                    let reveal_data_bytes = borsh::to_vec(&reveal_payload_data)?;
                    let commitment_hash = hex::encode(Sha256::digest(&reveal_data_bytes));

                    // Step 1: Commit
                    group_client
                        .commit_group_invitation(commitment_hash, expiration_block_height)
                        .await?;

                    // Step 2: Sign the reveal payload data with the joiner's key
                    let effective_key = effective_signing_key
                        .ok_or_else(|| eyre::eyre!("signing key required for commit/reveal"))?;
                    let joiner_private_key = PrivateKey::from(effective_key);
                    let hash = Sha256::digest(&reveal_data_bytes);
                    let signature = joiner_private_key
                        .sign(&hash)
                        .map_err(|e| eyre::eyre!("signing reveal payload failed: {e}"))?;
                    let invitee_signature = hex::encode(signature.to_bytes());

                    let signed_payload = SignedGroupRevealPayload {
                        data: reveal_payload_data,
                        invitee_signature,
                    };

                    // Step 3: Reveal
                    group_client.reveal_group_invitation(signed_payload).await?;
                }

                group_store::add_group_member(
                    &datastore,
                    &group_id,
                    &joiner_identity,
                    GroupMemberRole::Member,
                )?;

                let _ = node_client.subscribe_group(group_id.to_bytes()).await;
                let _ = node_client
                    .broadcast_group_mutation(group_id.to_bytes(), GroupMutationKind::MembersAdded)
                    .await;

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
