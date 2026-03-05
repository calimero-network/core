use std::time::{SystemTime, UNIX_EPOCH};

use actix::{ActorResponse, Handler, Message};
use calimero_context_config::types::{ContextGroupId, GroupInvitationFromAdmin, SignerId};
use calimero_context_primitives::group::{
    CreateGroupInvitationRequest, CreateGroupInvitationResponse,
};
use calimero_primitives::context::GroupInvitationPayload;
use calimero_primitives::identity::PrivateKey;
use rand::Rng;
use sha2::{Digest, Sha256};

use crate::{group_store, ContextManager};

impl Handler<CreateGroupInvitationRequest> for ContextManager {
    type Result = ActorResponse<Self, <CreateGroupInvitationRequest as Message>::Result>;

    fn handle(
        &mut self,
        CreateGroupInvitationRequest {
            group_id,
            requester,
            invitee_identity,
            expiration,
        }: CreateGroupInvitationRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            // 1. Group must exist
            let _meta = group_store::load_group_meta(&self.datastore, &group_id)?
                .ok_or_else(|| eyre::eyre!("group not found"))?;

            // 2. Requester must be admin
            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;

            // 3. Verify node holds the requester's signing key
            group_store::require_group_signing_key(&self.datastore, &group_id, &requester)?;

            // 4. Validate expiration (if set, must be in the future)
            if let Some(exp) = expiration {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if exp <= now {
                    eyre::bail!("expiration must be a future unix timestamp");
                }
            }

            // 5. Extract contract coordinates
            let params = self
                .external_config
                .params
                .get("near")
                .ok_or_else(|| eyre::eyre!("no 'near' protocol config"))?;

            // 6. Fetch admin signing key and construct + sign the invitation
            let signing_key_bytes =
                group_store::get_group_signing_key(&self.datastore, &group_id, &requester)?
                    .ok_or_else(|| eyre::eyre!("signing key not found for requester"))?;
            let private_key = PrivateKey::from(signing_key_bytes);

            let mut rng = rand::thread_rng();
            let secret_salt: [u8; 32] = rng.gen();

            // Use a large placeholder expiration block height (matches context invitation pattern).
            let expiration_block_height: u64 = 999_999_999;

            // Convert PublicKey to SignerId via borsh roundtrip (both are [u8; 32] wrappers).
            let requester_bytes: [u8; 32] = *requester;
            let inviter_signer_id: SignerId = borsh::from_slice(
                &borsh::to_vec(&requester_bytes)
                    .map_err(|e| eyre::eyre!("borsh serialize failed: {e}"))?,
            )
            .map_err(|e| eyre::eyre!("borsh deserialize to SignerId failed: {e}"))?;

            let config_group_id = ContextGroupId::from(group_id.to_bytes());

            let invitation = GroupInvitationFromAdmin {
                inviter_identity: inviter_signer_id,
                group_id: config_group_id,
                expiration_height: expiration_block_height,
                secret_salt,
                protocol: "near".to_string(),
                network: params.network.clone(),
                contract_id: params.contract_id.clone(),
            };

            // Sign: borsh-serialize → SHA256 → ed25519_sign
            let invitation_bytes = borsh::to_vec(&invitation)
                .map_err(|e| eyre::eyre!("failed to serialize invitation: {e}"))?;
            let hash = Sha256::digest(&invitation_bytes);
            let signature = private_key
                .sign(&hash)
                .map_err(|e| eyre::eyre!("signing failed: {e}"))?;
            let inviter_signature = hex::encode(signature.to_bytes());

            // 7. Build the invitation payload with the signature
            let payload = GroupInvitationPayload::new(
                group_id.to_bytes(),
                requester,
                invitee_identity,
                expiration,
                "near",
                &params.network,
                &params.contract_id,
                inviter_signature,
                secret_salt,
                expiration_block_height,
            )?;

            Ok(CreateGroupInvitationResponse { payload })
        })();

        ActorResponse::reply(result)
    }
}
