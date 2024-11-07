#![expect(single_use_lifetimes, reason = "borsh shenanigans")]

use std::borrow::Cow;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_crypto::SharedKey;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_storage::integration::Comparison;
use calimero_storage::interface::Action;
use serde::{Deserialize, Serialize};

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
