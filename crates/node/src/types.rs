use calimero_primitives::hash::Hash;
use calimero_primitives::transaction::Transaction;
use calimero_primitives::{application::ApplicationId, context::ContextId};
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub enum PeerAction {
    Transaction(Transaction),
    TransactionConfirmation(TransactionConfirmation),
    TransactionRejection(TransactionRejection),
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct TransactionConfirmation {
    pub context_id: ContextId,
    pub nonce: u64,
    pub transaction_hash: Hash,
    // sha256(previous_confirmation_hash, transaction_hash, nonce)
    pub confirmation_hash: Hash,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct TransactionRejection {
    pub context_id: ContextId,
    pub transaction_hash: Hash,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub enum CatchupStreamMessage {
    ApplicationBlobRequest(CatchupApplicationBlobRequest),
    ApplicationBlobSize(CatchupApplicationBlobSize),
    ApplicationBlobChunk(CatchupApplicationBlobChunk),
    TransactionsRequest(CatchupTransactionsRequest),
    TransactionsBatch(CatchupTransactionsBatch),
    Error(CatchupError),
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupApplicationBlobRequest {
    pub context_id: ApplicationId,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupApplicationBlobSize {
    pub size: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupApplicationBlobChunk(Box<[u8]>);

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupTransactionsRequest {
    pub context_id: ContextId,
    pub last_executed_transaction_hash: Hash,
    pub batch_size: u8,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupTransactionsBatch {
    pub transactions: Vec<TransactionWithStatus>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ThisError)]
#[non_exhaustive]
pub enum CatchupError {
    #[error("context `{context_id:?}` not found")]
    ContextNotFound { context_id: ContextId },
    #[error("transaction `{transaction_hash:?}` not found")]
    TransactionNotFound { transaction_hash: Hash },
    #[error("internal error")]
    InternalError,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct TransactionWithStatus {
    pub transaction_hash: Hash,
    pub transaction: Transaction,
    pub status: TransactionStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub enum TransactionStatus {
    Pending,
    Executed,
}
