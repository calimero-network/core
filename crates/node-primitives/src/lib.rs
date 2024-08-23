use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
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

pub type ServerSender = mpsc::Sender<(
    ContextId,
    String,
    Vec<u8>,
    bool,
    [u8; 32],
    oneshot::Sender<Result<Outcome, CallError>>,
)>;

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
