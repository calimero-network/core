use serde::{Deserialize, Serialize};

use calimero_store::entry::{Entry, Json};
use calimero_store::key::Generic;

#[derive(Debug, Serialize, Deserialize)]
pub enum PeerAction {
    Transaction(calimero_primitives::transaction::Transaction),
    TransactionConfirmation(TransactionConfirmation),
    CatchupRequest(CatchupRequest),
    CatchupResponse(CatchupResponse),
}

pub type Signature = Vec<u8>;

#[derive(Debug, Serialize, Deserialize)]
pub struct TransactionConfirmation {
    pub context_id: calimero_primitives::context::ContextId,
    pub nonce: u64,
    pub transaction_hash: calimero_primitives::hash::Hash,
    // sha256(previous_confirmation_hash, transaction_hash, nonce)
    pub confirmation_hash: calimero_primitives::hash::Hash,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CatchupRequest {
    pub context_id: calimero_primitives::context::ContextId,
    pub last_executed_transaction_hash: calimero_primitives::hash::Hash,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TransactionWithConfirmation {
    pub context_id: calimero_primitives::context::ContextId,
    pub transaction: calimero_primitives::transaction::Transaction,
    pub confirmation: TransactionConfirmation,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CatchupResponse {
    pub context_id: calimero_primitives::context::ContextId,
    pub transactions: Vec<TransactionWithConfirmation>,
}

#[derive(Serialize, Deserialize)]
pub struct SignedPeerAction {
    pub action: PeerAction,
    pub signature: Signature,
}

pub(crate) struct LastTxEntry {
    key: Generic,
}

impl Entry for LastTxEntry {
    type Key = Generic;
    type DataType<'a> = Json<calimero_primitives::hash::Hash>;

    fn key(&self) -> &Self::Key {
        &self.key
    }
}

impl LastTxEntry {
    pub(crate) fn new() -> Self {
        Self {
            key: Generic::new(*b"tx:calimero:node", [0; 32]),
        }
    }
}
