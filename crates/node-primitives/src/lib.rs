use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_runtime::logic::Outcome;
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub enum NodeType {
    Peer,
    Coordinator,
}

impl NodeType {
    #[must_use]
    pub const fn is_coordinator(&self) -> bool {
        match *self {
            Self::Coordinator => true,
            Self::Peer => false,
        }
    }
}

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
#[allow(clippy::exhaustive_enums)]
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
    Query(QueryCallError),
    Mutate(MutateCallError),
    ContextNotFound { context_id: ContextId },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ThisError)]
#[error("QueryCallError")]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum QueryCallError {
    ApplicationNotInstalled { application_id: ApplicationId },
    InternalError,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ThisError)]
#[error("MutateCallError")]
#[serde(tag = "type", content = "data")]
#[allow(variant_size_differences)]
#[non_exhaustive]
pub enum MutateCallError {
    InvalidNodeType { node_type: NodeType },
    ApplicationNotInstalled { application_id: ApplicationId },
    NoConnectedPeers,
    TransactionRejected,
    InternalError,
}
