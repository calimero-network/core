use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_runtime::logic::Outcome;
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug)]
#[non_exhaustive]
pub struct ExecutionRequest {
    pub context_id: ContextId,
    pub method: String,
    pub payload: Vec<u8>,
    pub executor_public_key: PublicKey,
    pub outcome_sender: oneshot::Sender<Result<Outcome, CallError>>,
    pub substitutes: Vec<Alias<PublicKey>>,
}

impl ExecutionRequest {
    #[must_use]
    pub const fn new(
        context_id: ContextId,
        method: String,
        payload: Vec<u8>,
        executor_public_key: PublicKey,
        outcome_sender: oneshot::Sender<Result<Outcome, CallError>>,
        substitutes: Vec<Alias<PublicKey>>,
    ) -> Self {
        Self {
            context_id,
            method,
            payload,
            executor_public_key,
            outcome_sender,
            substitutes,
        }
    }
}

pub type ServerSender = mpsc::Sender<ExecutionRequest>;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ThisError)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum CallError {
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
    #[error("error resolving identity alias '{alias}'")]
    AliasResolutionFailed { alias: Alias<PublicKey> },
}
