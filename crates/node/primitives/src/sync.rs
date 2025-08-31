#![expect(single_use_lifetimes, reason = "borsh shenanigans")]

use std::borrow::Cow;
use std::num::NonZeroUsize;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_crypto::Nonce;
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
        root_hash: Hash, // todo! shouldn't be cleartext
        artifact: Cow<'a, [u8]>,
        height: NonZeroUsize, // todo! shouldn't be cleartext
        nonce: Nonce,
    },
    // New batch message for multiple state deltas
    BatchStateDelta {
        context_id: ContextId,
        author_id: PublicKey,
        root_hash: Hash,
        deltas: Vec<BatchDelta<'a>>,
        nonce: Nonce,
    },
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct BatchDelta<'a> {
    pub artifact: Cow<'a, [u8]>,
    pub height: NonZeroUsize,
}

/// Optimized delta structure for efficient binary serialization
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct OptimizedDelta<'a> {
    /// Fixed-size header for fast parsing
    pub header: DeltaHeader,
    /// Raw payload data
    pub payload: Cow<'a, [u8]>,
}

/// Fixed-size header for optimized delta processing
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct DeltaHeader {
    /// Context ID (32 bytes)
    pub context_id: [u8; 32],
    /// Author ID (32 bytes)
    pub author_id: [u8; 32],
    /// Root hash (32 bytes)
    pub root_hash: [u8; 32],
    /// Height (4 bytes)
    pub height: u32,
    /// Timestamp (8 bytes)
    pub timestamp: u64,
    /// Flags for optimization hints
    pub flags: u8,
}

impl DeltaHeader {
    /// Create a new optimized delta header
    pub fn new(
        context_id: [u8; 32],
        author_id: [u8; 32],
        root_hash: [u8; 32],
        height: u32,
        timestamp: u64,
        flags: u8,
    ) -> Self {
        Self {
            context_id,
            author_id,
            root_hash,
            height,
            timestamp,
            flags,
        }
    }

    /// Check if this delta should use lightweight processing
    pub fn is_lightweight(&self) -> bool {
        self.flags & 0x01 != 0
    }

    /// Check if this delta should use direct P2P transmission
    pub fn use_direct_p2p(&self) -> bool {
        self.flags & 0x02 != 0
    }

    /// Check if this delta should be cached
    pub fn should_cache(&self) -> bool {
        self.flags & 0x04 != 0
    }
}

/// Optimized batch message for multiple deltas
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct OptimizedBatchDelta<'a> {
    /// Batch header
    pub header: BatchHeader,
    /// Multiple deltas in a single message
    pub deltas: Vec<OptimizedDelta<'a>>,
}

/// Header for batch delta messages
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct BatchHeader {
    /// Context ID (32 bytes)
    pub context_id: [u8; 32],
    /// Author ID (32 bytes)
    pub author_id: [u8; 32],
    /// Root hash (32 bytes)
    pub root_hash: [u8; 32],
    /// Number of deltas in batch
    pub delta_count: u16,
    /// Batch timestamp
    pub timestamp: u64,
    /// Batch flags
    pub flags: u8,
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub enum StreamMessage<'a> {
    Init {
        context_id: ContextId,
        party_id: PublicKey,
        payload: InitPayload,
        next_nonce: Nonce,
    },
    Message {
        sequence_id: usize,
        payload: MessagePayload<'a>,
        next_nonce: Nonce,
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
    DeltaSync {
        root_hash: Hash,
        application_id: ApplicationId,
    },
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub enum MessagePayload<'a> {
    StateSync {
        artifact: Cow<'a, [u8]>,
    },
    BlobShare {
        chunk: Cow<'a, [u8]>,
    },
    KeyShare {
        sender_key: PrivateKey,
    },
    DeltaSync {
        member: PublicKey,
        height: NonZeroUsize,
        delta: Option<Cow<'a, [u8]>>,
    },
}
