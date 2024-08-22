use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub enum NodeType {
    Peer,
    Coordinator,
}

impl NodeType {
    #[must_use]
    pub fn is_coordinator(&self) -> bool {
        match *self {
            Self::Coordinator => true,
            Self::Peer => false,
        }
    }
}

pub type ServerSender = mpsc::Sender<(
    calimero_primitives::context::ContextId,
    String,
    Vec<u8>,
    bool,
    [u8; 32],
    oneshot::Sender<Result<calimero_runtime::logic::Outcome, CallError>>,
)>;

#[derive(Clone, Copy, Debug, Deserialize, Error, Serialize)]
#[error("CallError")]
#[serde(tag = "type", content = "data")]
pub enum CallError {
    Query(QueryCallError),
    Mutate(MutateCallError),
    ContextNotFound {
        context_id: calimero_primitives::context::ContextId,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Error, Serialize)]
#[error("QueryCallError")]
#[serde(tag = "type", content = "data")]
pub enum QueryCallError {
    ApplicationNotInstalled {
        application_id: calimero_primitives::application::ApplicationId,
    },
    InternalError,
}

#[derive(Clone, Copy, Debug, Deserialize, Error, Serialize)]
#[error("MutateCallError")]
#[serde(tag = "type", content = "data")]
#[allow(variant_size_differences)]
pub enum MutateCallError {
    InvalidNodeType {
        node_type: NodeType,
    },
    ApplicationNotInstalled {
        application_id: calimero_primitives::application::ApplicationId,
    },
    NoConnectedPeers,
    TransactionRejected,
    InternalError,
}
