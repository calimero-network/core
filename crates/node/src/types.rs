use calimero_primitives::application::{ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PeerId;
use calimero_primitives::transaction::Transaction;
use semver::Version;
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
#[allow(variant_size_differences)]
pub enum CoordinatorCeremonyAction {
    Request(CoordinatorRequest),
    Offer(CoordinatorOffer),
    OfferAcceptance(CoordinatorOfferAcceptance),
    OfferRejection(CoordinatorOfferRejection),
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CoordinatorRequest {
    pub request_id: u64,
    pub context_id: ContextId,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CoordinatorOffer {
    pub request_id: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CoordinatorOfferAcceptance {
    pub request_id: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CoordinatorOfferRejection {
    pub request_id: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub enum CatchupStreamMessage {
    Request(CatchupRequest),
    ContextMetaChanged(CatchupContextMetaChanged),
    TransactionsBatch(CatchupTransactionBatch),
    Error(CatchupError),
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupRequest {
    pub context_id: ContextId,
    pub application_id: Option<ApplicationId>,
    pub last_executed_transaction_hash: Hash,
    pub coordinator_peer: Option<PeerId>,
    pub batch_size: u8,
}

// #[derive(Debug, Deserialize, Serialize)]
// #[non_exhaustive]
// pub struct CatchupCoordinatorChanged {
// }

// #[derive(Debug, Deserialize, Serialize)]
// #[non_exhaustive]
// pub struct CatchupContextMetaChanged {
//     pub application_id: ApplicationId,
//     pub blob_id: BlobId,
//     pub version: Option<Version>,
//     pub source: ApplicationSource,
//     pub hash: Option<Hash>,
//     pub metadata: Option<Vec<u8>>,
// }

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupContextMetaChanged {
    pub application_change: Option<CatchupApplicationChange>,
    pub coordinator_change: Option<CatchupCoordinatorChange>,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupApplicationChange {
    pub application_id: ApplicationId,
    pub blob_id: BlobId,
    pub version: Option<Version>,
    pub source: ApplicationSource,
    pub hash: Option<Hash>,
    pub metadata: Option<Vec<u8>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupCoordinatorChange {
    pub coordinator_peer: Option<PeerId>,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupTransactionBatch {
    pub transactions: Vec<TransactionWithStatus>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ThisError)]
#[non_exhaustive]
pub enum CatchupError {
    #[error("context `{context_id:?}` not found")]
    ContextNotFound { context_id: ContextId },
    #[error("context `{context_id:?}` does not have a coordinator")]
    ContextNotCoordinated { context_id: ContextId },
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
