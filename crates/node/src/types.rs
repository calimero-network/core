use serde::{Deserialize, Serialize};

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
    Response(CatchupResponse),
    Error(CatchupError),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CatchupRequest {
    pub context_id: calimero_primitives::context::ContextId,
    pub last_executed_transaction_hash: calimero_primitives::hash::Hash,
    pub batch_size: u8,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CatchupResponse {
    pub transactions: Vec<TransactionWithStatus>,
}

#[derive(Debug, Serialize, Deserialize)]
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
