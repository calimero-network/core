use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_storage::integration::Comparison;
use calimero_storage::interface::Action;
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
#[expect(clippy::large_enum_variant, reason = "Of no consequence here")]
pub enum PeerAction {
    ActionList(ActionMessage),
    Sync(SyncMessage),
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub enum CatchupStreamMessage {
    ActionsBatch(CatchupActionsBatch),
    ApplicationBlobRequest(CatchupApplicationBlobRequest),
    ApplicationBlobChunk(CatchupApplicationBlobChunk),
    SyncRequest(CatchupSyncRequest),
    Error(CatchupError),
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupApplicationBlobRequest {
    pub application_id: ApplicationId,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupApplicationBlobChunk {
    pub sequential_id: u64,
    pub chunk: Box<[u8]>,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupSyncRequest {
    pub context_id: ContextId,
    pub root_hash: Hash,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CatchupActionsBatch {
    pub actions: Vec<ActionMessage>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ThisError)]
#[non_exhaustive]
pub enum CatchupError {
    #[error("context `{context_id:?}` not found")]
    ContextNotFound { context_id: ContextId },
    #[error("application `{application_id:?}` not found")]
    ApplicationNotFound { application_id: ApplicationId },
    #[error("internal error")]
    InternalError,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ActionMessage {
    pub actions: Vec<Action>,
    pub context_id: ContextId,
    pub public_key: PublicKey,
    pub root_hash: Hash,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct SyncMessage {
    pub comparison: Comparison,
    pub context_id: ContextId,
    pub public_key: PublicKey,
    pub root_hash: Hash,
}
