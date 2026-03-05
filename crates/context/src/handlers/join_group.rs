use std::time::{SystemTime, UNIX_EPOCH};

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_config::types::{
    ContextGroupId, GroupInvitationFromAdmin, GroupRevealPayloadData, SignedGroupOpenInvitation,
    SignedGroupRevealPayload, SignerId,
};
use calimero_context_primitives::group::{JoinGroupRequest, JoinGroupResponse};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use eyre::bail;
use sha2::{Digest, Sha256};
use tracing::info;

use crate::{group_store, ContextManager};

impl Handler<JoinGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <JoinGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinGroupRequest {
            invitation_payload,
            joiner_identity,
            signing_key,
        }: JoinGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Decode invitation (now includes inviter_signature, secret_salt, expiration_block_height)
        let (
            group_id_bytes,
            inviter_identity,
            invitee_identity,
            expiration,
            protocol,
            network_id,
            contract_id,
            inviter_signature,
            secret_salt,
            expiration_block_height,
        ) = match invitation_payload.parts() {
            Ok(parts) => parts,
            Err(err) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "failed to decode group invitation payload: {err}"
                )));
            }
        };

        let group_id = ContextGroupId::from(group_id_bytes);

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

        // Resolve effective signing key (provided or previously stored)
        let effective_signing_key = match signing_key {
            Some(sk) => Some(sk),
            None => {
                group_store::get_group_signing_key(&self.datastore, &group_id, &joiner_identity)
                    .ok()
                    .flatten()
            }
        };

        let group_client_result = effective_signing_key.map(|sk| self.group_client(group_id, sk));

        let datastore = self.datastore.clone();
        let context_client = self.context_client.clone();

        ActorResponse::r#async(
            async move {
                // Phase 1: Bootstrap from chain if local state is missing
                if needs_chain_sync {
                    let mut meta = group_store::sync_group_state_from_contract(
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

                    // Add inviter as admin locally so validation passes
                    group_store::add_group_member(
                        &datastore,
                        &group_id,
                        &inviter_identity,
                        GroupMemberRole::Admin,
                    )?;
                }

                // Phase 2: Validate
                if !group_store::is_group_admin(&datastore, &group_id, &inviter_identity)? {
                    bail!("inviter is no longer an admin of this group");
                }

                if let Some(expected_invitee) = invitee_identity {
                    if expected_invitee != joiner_identity {
                        bail!("this invitation is for a different identity");
                    }
                }

                if let Some(exp) = expiration {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    if now > exp {
                        bail!("invitation has expired");
                    }
                }

                if group_store::check_group_membership(&datastore, &group_id, &joiner_identity)? {
                    bail!("identity is already a member of this group");
                }

                // Phase 3: Contract commit/reveal + local store
                if let Some(client_result) = group_client_result {
                    let group_client = client_result?;

                    // Convert identities to contract types via borsh roundtrip
                    let inviter_bytes: [u8; 32] = *inviter_identity;
                    let inviter_signer_id: SignerId =
                        borsh::from_slice(&borsh::to_vec(&inviter_bytes)?)?;

                    let joiner_bytes: [u8; 32] = *joiner_identity;
                    let new_member_signer_id: SignerId =
                        borsh::from_slice(&borsh::to_vec(&joiner_bytes)?)?;

                    // Reconstruct the GroupInvitationFromAdmin
                    let invitation = GroupInvitationFromAdmin {
                        inviter_identity: inviter_signer_id,
                        group_id,
                        expiration_height: expiration_block_height,
                        secret_salt,
                        protocol: protocol.clone(),
                        network: network_id.clone(),
                        contract_id: contract_id.clone(),
                    };

                    let signed_open_invitation = SignedGroupOpenInvitation {
                        invitation,
                        inviter_signature,
                    };

                    // Build the reveal payload data
                    let reveal_payload_data = GroupRevealPayloadData {
                        signed_open_invitation,
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
