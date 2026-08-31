use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{CreateGroupInvitationRequest, CreateGroupInvitationResponse};
use calimero_context_config::types::{
    GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
};
use calimero_context_config::MemberCapabilities;
use calimero_governance_store::{MembershipRepository, MetaRepository, MetadataRepository};
use calimero_primitives::identity::PrivateKey;
use rand::Rng;
use sha2::{Digest, Sha256};

use crate::ContextManager;

impl Handler<CreateGroupInvitationRequest> for ContextManager {
    type Result = ActorResponse<Self, <CreateGroupInvitationRequest as Message>::Result>;

    fn handle(
        &mut self,
        CreateGroupInvitationRequest {
            group_id,
            expiration_timestamp,
            admitters,
        }: CreateGroupInvitationRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let (signer, node_sk) = match self.resolve_signer(&group_id) {
            Ok(pair) => pair,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let datastore = self.datastore.clone();

        let result = (|| -> eyre::Result<_> {
            let meta = MetaRepository::new(&datastore)
                .load(&group_id)?
                .ok_or_else(|| eyre::eyre!("group not found"))?;

            let signer_account = crate::member_account::require(&datastore, &group_id, &signer)?;
            MembershipRepository::new(&datastore).require_admin_or_capability(
                &group_id,
                &signer_account,
                MemberCapabilities::CAN_INVITE_MEMBERS.bits(),
                "create group invitation",
            )?;

            let private_key = PrivateKey::from(node_sk);

            let mut rng = rand::thread_rng();
            let invitation_nonce: [u8; 32] = rng.gen();

            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_secs();
            // Clamped, not merely defaulted: a caller asking for longer is a
            // caller extending how long a leaked bearer credential stays
            // redeemable, and the request is not theirs to grant.
            let requested = expiration_timestamp
                .unwrap_or(calimero_context_config::types::MAX_INVITATION_VALIDITY_SECS)
                .min(calimero_context_config::types::MAX_INVITATION_VALIDITY_SECS);
            let expiration_timestamp: u64 = now_secs + requested;

            let inviter_signer_id = SignerId::from(*signer);

            let invitation = GroupInvitationFromAdmin {
                inviter_identity: inviter_signer_id,
                group_id,
                expiration_timestamp,
                invitation_nonce,
                invited_role: 1, // Member
                admitters,
            };

            let invitation_bytes = borsh::to_vec(&invitation)
                .map_err(|e| eyre::eyre!("failed to serialize invitation: {e}"))?;
            let hash = Sha256::digest(&invitation_bytes);
            let signature = private_key
                .sign(&hash)
                .map_err(|e| eyre::eyre!("signing failed: {e}"))?;
            let inviter_signature = hex::encode(signature.to_bytes());

            let group_name = MetadataRepository::new(&datastore)
                .group_metadata(&group_id)?
                .and_then(|r| r.name);

            Ok((
                SignedGroupOpenInvitation {
                    inviter_account: Some(signer_account),
                    invitation,
                    inviter_signature,
                    // Carry the real application_id so the joiner can
                    // pre-populate GroupMetaValue correctly. Without this,
                    // joiners would write target_application_id = ZERO
                    // and compute_group_state_hash would diverge from
                    // the inviter's view persistently.
                    application_id: Some(*meta.target_application_id.as_ref()),
                    // Carry the real bytecode_id (already derived from
                    // blob_id(app_meta.bytecode) at create_group time)
                    // so the joiner's pre-populated GroupMetaValue
                    // matches the originator's. Without this the
                    // joiner's bytecode_id seeds to [0u8; 32] and any
                    // CascadeUpgrade op the joiner
                    // applies silently skips the subtree — divergence
                    // between originator and joiner.
                    bytecode_id: Some(meta.bytecode_id),
                    admitter_hints: Vec::new(),
                },
                group_name,
            ))
        })();

        let (signed_invitation, group_name) = match result {
            Ok(v) => v,
            Err(e) => return ActorResponse::reply(Err(e)),
        };

        // No commitment publishing needed — the signed invitation is a
        // self-contained bearer credential. The joiner will present it
        // in a RootOp::MemberJoined on the namespace topic.
        ActorResponse::reply(Ok(CreateGroupInvitationResponse {
            invitation: signed_invitation,
            group_name,
        }))
    }
}
