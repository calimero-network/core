#![expect(single_use_lifetimes, reason = "borsh shenanigans")]

use std::borrow::Cow;
use std::num::NonZeroUsize;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_crypto::Nonce;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;

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
    },
    /// Batch of multiple state deltas for efficiency
    BatchStateDelta {
        context_id: ContextId,
        author_id: PublicKey,
        root_hash: Hash,
        deltas: Vec<BatchDelta<'a>>,
        nonce: Nonce,
    },
}

/// Individual delta within a batch
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct BatchDelta<'a> {
    pub artifact: Cow<'a, [u8]>,
    pub height: NonZeroUsize,
}
