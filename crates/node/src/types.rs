#![expect(single_use_lifetimes, reason = "borsh shenanigans")]

use std::borrow::Cow;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_crypto::NONCE_LEN;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};

#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[non_exhaustive]
#[expect(clippy::large_enum_variant, reason = "Of no consequence here")]
pub enum BroadcastMessage<'a> {
    StateDelta {
        context_id: ContextId,
        author_id: PublicKey,
        root_hash: Hash,
        artifact: Cow<'a, [u8]>,
        nonce: [u8; NONCE_LEN],
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
    KeyShare,
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[expect(variant_size_differences, reason = "'tis fine")]
pub enum MessagePayload<'a> {
    StateSync { artifact: Cow<'a, [u8]> },
    BlobShare { chunk: Cow<'a, [u8]> },
    KeyShare { sender_key: PrivateKey },
}
