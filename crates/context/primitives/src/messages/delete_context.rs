use actix::Message;
use calimero_primitives::context::ContextId;

#[derive(Copy, Clone, Debug)]
pub struct DeleteContextRequest {
    pub context_id: ContextId,
}

impl Message for DeleteContextRequest {
    type Result = eyre::Result<DeleteContextResponse>;
}

#[derive(Copy, Clone, Debug)]
pub struct DeleteContextResponse {
    pub deleted: bool,
}
