use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum NodeType {
    Peer,
    Coordinator,
}

impl NodeType {
    pub fn is_coordinator(&self) -> bool {
        match *self {
            NodeType::Coordinator => true,
            NodeType::Peer => false,
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

#[derive(Clone, Debug, Serialize, Deserialize, Error)]
#[error("CallError")]
#[serde(tag = "type", content = "data")]
pub enum CallError {
    Query(QueryCallError),
    Mutate(MutateCallError),
    ContextNotFound {
        context_id: calimero_primitives::context::ContextId,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, Error)]
#[error("QueryCallError")]
#[serde(tag = "type", content = "data")]
pub enum QueryCallError {
    ApplicationNotInstalled {
        application_id: calimero_primitives::application::ApplicationId,
    },
    InternalError,
}

#[derive(Clone, Debug, Serialize, Deserialize, Error)]
#[error("MutateCallError")]
#[serde(tag = "type", content = "data")]
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
