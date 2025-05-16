use actix::Message;
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::identity::{PrivateKey, PublicKey};

#[derive(Debug)]
pub struct JoinContextRequest {
    pub identity_secret: PrivateKey,
    pub invitation_payload: ContextInvitationPayload,
}

impl Message for JoinContextRequest {
    type Result = eyre::Result<JoinContextResponse>;
}

#[derive(Copy, Clone, Debug)]
pub struct JoinContextResponse {
    pub context_id: ContextId,
    pub member_public_key: PublicKey,
}
