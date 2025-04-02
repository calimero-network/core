use actix::Message;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;

#[derive(Debug)]
pub struct UpdateApplicationRequest {
    pub context_id: ContextId,
    pub application_id: ApplicationId,
    pub public_key: PublicKey,
}

impl Message for UpdateApplicationRequest {
    type Result = eyre::Result<()>;
}
