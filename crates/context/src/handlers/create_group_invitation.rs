use actix::{ActorResponse, Handler, Message};
use calimero_context_config::types::{
    GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
};
use calimero_context_config::MemberCapabilities;
use calimero_context_primitives::group::{
    CreateGroupInvitationRequest, CreateGroupInvitationResponse,
};
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
            expiration_timestamp,
        }: CreateGroupInvitationRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_group_identity();

        // Resolve requester: use provided value or fall back to node group identity
        let requester = match requester {
            Some(pk) => pk,
            None => match node_identity {
                Some((pk, _)) => pk,
                None => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "requester not provided and node has no configured group identity"
                    )))
                }
            },
        };

        // Auto-store node signing key ONLY when the requester IS the node's own identity
        if let Some((node_pk, node_sk)) = node_identity {
            if requester == node_pk {
                let _ = group_store::store_group_signing_key(
                    &self.datastore,
                    &group_id,
                    &requester,
                    &node_sk,
                );
            }
        }

        let result = (|| {
            // 1. Group must exist
            let _meta = group_store::load_group_meta(&self.datastore, &group_id)?
                .ok_or_else(|| eyre::eyre!("group not found"))?;

            // 2. Requester must be admin or hold CAN_INVITE_MEMBERS capability
            group_store::require_group_admin_or_capability(
                &self.datastore,
                &group_id,
                &requester,
                MemberCapabilities::CAN_INVITE_MEMBERS,
                "create group invitation",
            )?;

            // 3. Verify node holds the requester's signing key
            group_store::require_group_signing_key(&self.datastore, &group_id, &requester)?;

            // 4. Fetch admin signing key and construct + sign the invitation
            let signing_key_bytes =
                group_store::get_group_signing_key(&self.datastore, &group_id, &requester)?
                    .ok_or_else(|| eyre::eyre!("signing key not found for requester"))?;
            let private_key = PrivateKey::from(signing_key_bytes);

            let mut rng = rand::thread_rng();
            let secret_salt: [u8; 32] = rng.gen();

            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_secs();
            let expiration_timestamp: u64 =
                now_secs + expiration_timestamp.unwrap_or(365 * 24 * 3600);

            let inviter_signer_id = SignerId::from(*requester);

            let invitation = GroupInvitationFromAdmin {
                inviter_identity: inviter_signer_id,
                group_id,
                expiration_timestamp,
                secret_salt,
            };

            // Sign: borsh-serialize → SHA256 → ed25519_sign
            let invitation_bytes = borsh::to_vec(&invitation)
                .map_err(|e| eyre::eyre!("failed to serialize invitation: {e}"))?;
            let hash = Sha256::digest(&invitation_bytes);
            let signature = private_key
                .sign(&hash)
                .map_err(|e| eyre::eyre!("signing failed: {e}"))?;
            let inviter_signature = hex::encode(signature.to_bytes());

            let group_alias = group_store::get_group_alias(&self.datastore, &group_id)?;

            Ok(CreateGroupInvitationResponse {
                invitation: SignedGroupOpenInvitation {
                    invitation,
                    inviter_signature,
                },
                group_alias,
            })
        })();

        ActorResponse::reply(result)
    }
}
