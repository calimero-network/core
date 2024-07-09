use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Serialize, Deserialize)]
pub enum PeerAction {
    Transaction(calimero_primitives::transaction::Transaction),
    TransactionConfirmation(TransactionConfirmation),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TransactionConfirmation {
    pub context_id: calimero_primitives::context::ContextId,
    pub nonce: u64,
    pub transaction_hash: calimero_primitives::hash::Hash,
    // sha256(previous_confirmation_hash, transaction_hash, nonce)
    pub confirmation_hash: calimero_primitives::hash::Hash,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum CatchupStreamMessage {
    Request(CatchupRequest),
    ApplicationChanged(CatchupApplicationChanged),
    TransactionsBatch(CatchupTransactionBatch),
    Error(CatchupError),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CatchupRequest {
    pub context_id: calimero_primitives::context::ContextId,
    pub application_id: Option<calimero_primitives::application::ApplicationId>,
    pub last_executed_transaction_hash: calimero_primitives::hash::Hash,
    pub batch_size: u8,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CatchupResponseMeta {
    pub application_id: calimero_primitives::application::ApplicationId,
    pub version: semver::Version,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CatchupApplicationChanged {
    pub application_id: calimero_primitives::application::ApplicationId,
    pub version: semver::Version,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CatchupTransactionBatch {
    pub transactions: Vec<TransactionWithStatus>,
}

#[derive(Error, Debug, Serialize, Deserialize)]
#[error("CatchupError")]
pub enum CatchupError {
    ContextNotFound {
        context_id: calimero_primitives::context::ContextId,
    },
    TransactionNotFound {
        transaction_hash: calimero_primitives::hash::Hash,
    },
    InternalError,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TransactionWithStatus {
    pub transaction_hash: calimero_primitives::hash::Hash,
    pub transaction: calimero_primitives::transaction::Transaction,
    pub status: TransactionStatus,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum TransactionStatus {
    Pending,
    Executed,
}
