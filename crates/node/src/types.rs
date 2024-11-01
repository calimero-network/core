use std::borrow::Cow;
use std::fmt::Result;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_primitives::{application::ApplicationId, context::Context};
use calimero_runtime::logic::Outcome;
use calimero_storage::integration::Comparison;
use calimero_storage::interface::Action;
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

// use crate::Node;

// mod v0;

// // pub mod current {
// //     use super::v0 as imp;

// //     pub type WireMessage = imp::WireMessage;
// //     pub type WirePayload = imp::WirePayload;
// // }

// // trait Handle<WireMessage: Message> {
// //     fn handle(&self, message: WireMessage) -> eyre::Result<()>;
// // }

// trait Environment {
//     type Params;
// }

// struct Broadcast;
// struct BroadcastParams<'a> {
//     pub context: &'a Context,
//     pub public_key: PublicKey,
//     pub outcome: &'a Outcome,
// }

// pub enum StreamMessage<'a> {
//     Initialize {
//         context_id: ContextId,
//         public_key: PublicKey,
//     },
//     SyncAction {
//         method: Cow<'a, str>,
//     },
// }

// impl Environment for Broadcast {
//     type Params = ();
// }

// trait Message<Environment> {
//     const VERSION: usize;

//     fn new(
//         node: &Node,
//         context: &Context,
//         executor: PublicKey,
//         artifacts: Vec<u8>,
//     ) -> eyre::Result<Self>;
// }

// trait Upgrade: Message {
//     type Upgraded: Message;
//     fn upgrade(self) -> eyre::Result<Self::Upgraded>;
// }

// // impl<T> Upgrade<T> for T {
// //     fn upgrade(self) -> eyre::Result<T> {
// //         Ok(self)
// //     }
// // }

// #[derive(Debug, BorshSerialize, BorshDeserialize)]
// pub enum WireMessage {
//     V0(v0::WireMessage),
// }

// impl Message for WireMessage {
//     const VERSION: usize = usize::MAX;

//     fn new(
//         node: &Node,
//         context: &Context,
//         executor: PublicKey,
//         artifacts: Vec<u8>,
//     ) -> eyre::Result<Self> {
//         let this = match context.wire_version {
//             v0::WireMessage::VERSION => {
//                 WireMessage::V0(v0::WireMessage::new(node, context, executor, artifacts)?)
//             }
//         };

//         Ok(this)
//     }
// }

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

#[derive(Debug, BorshSerialize, BorshDeserialize)]
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
    StateSync {
        method: Cow<'a, str>,
        artifact: Cow<'a, [u8]>,
    },
    BlobShare {
        chunk: Cow<'a, [u8]>,
    },
}

// struct StateSync;

// trait Message<'a> {
//     type Init;
//     type Message;
// }

// impl<'a> Message<'a> for StateSync {
//     type Init = StateSyncHandshake<'a>;
//     type Message = StateSyncAction<'a>;
// }

// #[derive(Debug, BorshSerialize, BorshDeserialize)]
// #[non_exhaustive]
// pub enum DirectMessage<'a> {
//     StateSync(StreamMessage<StateSyncHandshake<'a>, StateSyncAction<'a>>),
//     BlobShare(StreamMessage<BlobShareRequest<'a>, BlobShareChunk<'a>>),
// }

// #[derive(Debug, BorshSerialize, BorshDeserialize)]
// pub struct StateSyncAction<'a> {}

// #[derive(Debug, BorshSerialize, BorshDeserialize)]
// pub struct BlobShareRequest<'a> {
//     blob_id: BlobId,
// }

// #[derive(Debug, BorshSerialize, BorshDeserialize)]
// pub struct BlobShareChunk<'a> {}

// #[derive(Debug, Deserialize, Serialize)]
// #[non_exhaustive]
// pub enum CatchupStreamMessage {
//     ActionsBatch(CatchupActionsBatch),
//     ApplicationBlobRequest(CatchupApplicationBlobRequest),
//     ApplicationBlobChunk(CatchupApplicationBlobChunk),
//     SyncRequest(CatchupSyncRequest),
//     Error(BlobError),
// }

// #[derive(Debug, Deserialize, Serialize)]
// #[non_exhaustive]
// pub struct CatchupApplicationBlobRequest {
//     pub application_id: ApplicationId,
// }

// #[derive(Debug, Deserialize, Serialize)]
// #[non_exhaustive]
// pub struct CatchupApplicationBlobChunk {
//     pub sequential_id: u64,
//     pub chunk: Box<[u8]>,
// }

// #[derive(Debug, Deserialize, Serialize)]
// #[non_exhaustive]
// pub struct CatchupSyncRequest {
//     pub context_id: ContextId,
//     pub root_hash: Hash,
// }

// #[derive(Debug, Deserialize, Serialize)]
// #[non_exhaustive]
// pub struct CatchupActionsBatch {
//     pub actions: Vec<ActionMessage>,
// }

// #[derive(Clone, Copy, Debug, Deserialize, Serialize, ThisError)]
// #[non_exhaustive]
// pub enum BlobError {
//     #[error("context `{context_id:?}` not found")]
//     ContextNotFound { context_id: ContextId },
//     #[error("application `{application_id:?}` not found")]
//     ApplicationNotFound { application_id: ApplicationId },
//     #[error("internal error")]
//     InternalError,
// }

// #[derive(Clone, Debug, Deserialize, Serialize)]
// #[non_exhaustive]
// pub struct ActionMessage {
//     pub actions: Vec<Action>,
//     pub context_id: ContextId,
//     pub public_key: PublicKey,
//     pub root_hash: Hash,
// }

// #[derive(Clone, Debug, Deserialize, Serialize)]
// #[non_exhaustive]
// pub struct SyncMessage {
//     pub comparison: Comparison,
//     pub context_id: ContextId,
//     pub public_key: PublicKey,
//     pub root_hash: Hash,
// }
