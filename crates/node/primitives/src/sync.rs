#![expect(single_use_lifetimes, reason = "borsh shenanigans")]

use std::borrow::Cow;
use std::num::NonZeroUsize;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_crypto::Nonce;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_primitives::blobs::BlobId;

use crate::clock::Hlc;

/// Core broadcast message types for state synchronization
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[non_exhaustive]
#[expect(clippy::large_enum_variant, reason = "Of no consequence here")]
pub enum BroadcastMessage<'a> {
    /// Single state delta broadcast
    StateDelta {
        context_id: ContextId,
        author_id: PublicKey,
        root_hash: Hash,
        artifact: Cow<'a, [u8]>,
        height: NonZeroUsize,
        nonce: Nonce,
        /// Hybrid Logical Clock timestamp for causal ordering
        timestamp: Hlc,
    },
    /// Batch of multiple state deltas for efficiency
    BatchStateDelta {
        context_id: ContextId,
        author_id: PublicKey,
        root_hash: Hash,
        deltas: Vec<BatchDelta<'a>>,
        nonce: Nonce,
        /// Hybrid Logical Clock timestamp for causal ordering
        timestamp: Hlc,
    },
}

/// Individual delta within a batch
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct BatchDelta<'a> {
    pub artifact: Cow<'a, [u8]>,
    pub height: NonZeroUsize,
    /// Hybrid Logical Clock timestamp for causal ordering
    pub timestamp: Hlc,
}

/// Stream message types for P2P communication
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub enum StreamMessage<'a> {
    Init {
        context_id: ContextId,
        party_id: PublicKey,
        payload: InitPayload,
        next_nonce: [u8; 12],
    },
    Message {
        sequence_id: u64,
        payload: MessagePayload<'a>,
        next_nonce: [u8; 12],
    },
    OpaqueError,
}

/// Initialization payload for stream setup
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub enum InitPayload {
    DeltaSync { 
        context_id: ContextId,
        root_hash: Hash,
        application_id: calimero_primitives::application::ApplicationId,
    },
    StateSync { 
        context_id: ContextId,
        root_hash: Hash,
        application_id: calimero_primitives::application::ApplicationId,
    },
    BlobShare { blob_id: BlobId },
    KeyShare,
}

/// Message payload for ongoing communication
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub enum MessagePayload<'a> {
    DeltaSync { 
        member: PublicKey,
        height: NonZeroUsize,
        delta: Option<Cow<'a, [u8]>>,
        artifact: Cow<'a, [u8]>,
    },
    StateSync { artifact: Cow<'a, [u8]> },
    BlobShare { chunk: Cow<'a, [u8]> },
    KeyShare { sender_key: Cow<'a, [u8]> },
}
