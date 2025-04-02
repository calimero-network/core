use actix::Message;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_runtime::logic::Outcome;
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

#[derive(Debug)]
pub struct ExecuteRequest {
    pub context_id: ContextId,
    pub method: String,
    pub payload: Vec<u8>,
    pub public_key: PublicKey,
}

impl Message for ExecuteRequest {
    type Result = Result<Outcome, ExecutionError>;
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ThisError)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum ExecutionError {
    #[error("context not found")]
    ContextNotFound,
    #[error("cannot execute request as '{public_key}' on context '{context_id}'")]
    Unauthorized {
        context_id: ContextId,
        public_key: PublicKey,
    },
    #[error("context state not initialized, awaiting state sync")]
    Uninitialized,
    #[error("application not installed: '{application_id}'")]
    ApplicationNotInstalled { application_id: ApplicationId },
    #[error("internal error")]
    InternalError,
}
