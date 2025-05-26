use actix::Message;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{PrivateKey, PublicKey};

#[derive(Debug)]
pub struct CreateContextRequest {
    pub protocol: String,
    pub seed: Option<[u8; 32]>,
    pub application_id: ApplicationId,
    pub identity_secret: Option<PrivateKey>,
    pub init_params: Vec<u8>,
}

impl Message for CreateContextRequest {
    type Result = eyre::Result<CreateContextResponse>;
}

#[derive(Copy, Clone, Debug)]
pub struct CreateContextResponse {
    pub context_id: ContextId,
    pub identity: PublicKey,
}
