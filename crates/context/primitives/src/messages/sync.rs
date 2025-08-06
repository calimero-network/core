use actix::Message;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;

#[derive(Copy, Clone, Debug)]
pub struct SyncRequest {
    pub context_id: ContextId,
    pub application_id: ApplicationId,
}

impl Message for SyncRequest {
    type Result = ();
}
