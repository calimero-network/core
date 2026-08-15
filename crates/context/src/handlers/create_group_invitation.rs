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
            requester,
            expiration_timestamp,
        }: CreateGroupInvitationRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let (requester, node_sk) = match self.resolve_signer(&group_id, requester) {
            Ok(pair) => pair,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let datastore = self.datastore.clone();

        let result = (|| -> eyre::Result<_> {
            let meta = MetaRepository::new(&datastore)
                .load(&group_id)?
                .ok_or_else(|| eyre::eyre!("group not found"))?;

            let requester_account =
                crate::member_account::require(&datastore, &group_id, &requester)?;
            MembershipRepository::new(&datastore).require_admin_or_capability(
                &group_id,
                &requester_account,
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
            let expiration_timestamp: u64 =
                now_secs + expiration_timestamp.unwrap_or(365 * 24 * 3600);

            let inviter_signer_id = SignerId::from(*requester);

            let invitation = GroupInvitationFromAdmin {
                inviter_identity: inviter_signer_id,
                group_id,
                expiration_timestamp,
                invitation_nonce,
                invited_role: 1, // Member
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
                    inviter_account: Some(requester_account),
                    invitation,
                    inviter_signature,
                    // Carry the real application_id so the joiner can
                    // pre-populate GroupMetaValue correctly. Without this,
                    // joiners would write target_application_id = ZERO
                    // and compute_group_state_hash would diverge from
                    // the inviter's view persistently.
                    application_id: Some(*meta.target_application_id.as_ref()),
                    // Carry the real app_key (already derived from
                    // blob_id(app_meta.bytecode) at create_group time)
                    // so the joiner's pre-populated GroupMetaValue
                    // matches the originator's. Without this the
                    // joiner's app_key seeds to [0u8; 32] and any
                    // CascadeUpgrade op the joiner
                    // applies silently skips the subtree — divergence
                    // between originator and joiner.
                    app_key: Some(meta.app_key),
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
