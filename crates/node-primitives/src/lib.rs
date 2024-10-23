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
    pub finality: Option<Finality>,
}

impl ExecutionRequest {
    #[must_use]
    pub const fn new(
        context_id: ContextId,
        method: String,
        payload: Vec<u8>,
        executor_public_key: PublicKey,
        outcome_sender: oneshot::Sender<Result<Outcome, CallError>>,
        finality: Option<Finality>,
    ) -> Self {
        Self {
            context_id,
            method,
            payload,
            executor_public_key,
            outcome_sender,
            finality,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "There will never be any other variants"
)]
pub enum Finality {
    Local,
    Global,
}

pub type ServerSender = mpsc::Sender<ExecutionRequest>;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ThisError)]
#[error("CallError")]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum CallError {
    ApplicationNotInstalled { application_id: ApplicationId },
    NoConnectedPeers,
    ActionRejected,
    InternalError,
    ContextNotFound { context_id: ContextId },
}
