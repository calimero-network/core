#![expect(single_use_lifetimes, reason = "borsh shenanigans")]

use std::borrow::Cow;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_crypto::SharedKey;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;

#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[non_exhaustive]
#[expect(clippy::large_enum_variant, reason = "Of no consequence here")]
pub enum BroadcastMessage<'a> {
    StateDelta {
        context_id: ContextId,
        author_id: PublicKey,
        root_hash: Hash,
        artifact: Cow<'a, [u8]>,
    },
}
pub enum PeerAction {
    ActionList(ActionMessage),
    Sync(SyncMessage),
    RequestSenderKey(RequestSenderKeyMessage),
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

#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub enum StreamMessage<'a> {
    Init {
        context_id: ContextId,
        party_id: PublicKey,
        // nonce: usize,
        payload: InitPayload,
    },
    Message {
        sequence_id: usize,
        payload: MessagePayload<'a>,
    },
    /// Other peers must not learn anything about the node's state if anything goes wrong.
    OpaqueError,
}

#[derive(Copy, Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum InitPayload {
    BlobShare {
        blob_id: BlobId,
    },
    StateSync {
        root_hash: Hash,
        application_id: ApplicationId,
    },
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub enum MessagePayload<'a> {
    StateSync { artifact: Cow<'a, [u8]> },
    BlobShare { chunk: Cow<'a, [u8]> },
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct RequestSenderKeyMessage {
    pub context_id: ContextId,
    pub public_key: PublicKey,
    pub shared_key: SharedKey,
}
