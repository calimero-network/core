use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub enum PeerAction {
    Transaction(calimero_primitives::transaction::Transaction),
    TransactionConfirmation(TransactionConfirmation),
    TransactionRejection(TransactionRejection),
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct TransactionConfirmation {
    pub context_id: calimero_primitives::context::ContextId,
    pub nonce: u64,
    pub transaction_hash: calimero_primitives::hash::Hash,
    // sha256(previous_confirmation_hash, transaction_hash, nonce)
    pub confirmation_hash: calimero_primitives::hash::Hash,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct TransactionRejection {
    pub context_id: calimero_primitives::context::ContextId,
    pub transaction_hash: calimero_primitives::hash::Hash,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub enum CatchupStreamMessage {
    Request(CatchupRequest),
    ApplicationChanged(CatchupApplicationChanged),
    TransactionsBatch(CatchupTransactionBatch),
    Error(CatchupError),
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupRequest {
    pub context_id: calimero_primitives::context::ContextId,
    pub application_id: Option<calimero_primitives::application::ApplicationId>,
    pub last_executed_transaction_hash: calimero_primitives::hash::Hash,
    pub batch_size: u8,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupApplicationChanged {
    pub application_id: calimero_primitives::application::ApplicationId,
    pub blob_id: calimero_primitives::blobs::BlobId,
    pub version: Option<semver::Version>,
    pub source: calimero_primitives::application::ApplicationSource,
    pub hash: Option<calimero_primitives::hash::Hash>,
    pub metadata: Option<Vec<u8>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupTransactionBatch {
    pub transactions: Vec<TransactionWithStatus>,
}

#[derive(Clone, Copy, Debug, Deserialize, Error, Serialize)]
#[non_exhaustive]
pub enum CatchupError {
    #[error("context `{context_id:?}` not found")]
    ContextNotFound {
        context_id: calimero_primitives::context::ContextId,
    },
    #[error("transaction `{transaction_hash:?}` not found")]
    TransactionNotFound {
        transaction_hash: calimero_primitives::hash::Hash,
    },
    #[error("internal error")]
    InternalError,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct TransactionWithStatus {
    pub transaction_hash: calimero_primitives::hash::Hash,
    pub transaction: calimero_primitives::transaction::Transaction,
    pub status: TransactionStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub enum TransactionStatus {
    Pending,
    Executed,
}
