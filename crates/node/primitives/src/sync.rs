#![expect(single_use_lifetimes, reason = "borsh shenanigans")]

use std::borrow::Cow;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_crypto::Nonce;
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

        /// DAG: Unique delta ID (content hash)
        delta_id: [u8; 32],

        /// DAG: Parent delta IDs (for causal ordering)
        parent_ids: Vec<[u8; 32]>,

        /// Hybrid Logical Clock timestamp for causal ordering
        hlc: calimero_storage::logical_clock::HybridTimestamp,

        root_hash: Hash, // todo! shouldn't be cleartext
        artifact: Cow<'a, [u8]>,
        nonce: Nonce,

        /// Execution events that were emitted during the state change.
        /// This field is encrypted along with the artifact.
        events: Option<Cow<'a, [u8]>>,
    },

    /// Hash heartbeat for divergence detection
    ///
    /// Periodically broadcast by nodes to allow peers to detect silent divergence.
    /// If a peer has a different hash for the same DAG heads, it indicates a problem.
    HashHeartbeat {
        context_id: ContextId,
        /// Current root hash
        root_hash: Hash,
        /// Current DAG head(s)
        dag_heads: Vec<[u8; 32]>,
    },
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
    KeyShare,
    /// Request a specific delta by ID (for DAG gap filling)
    DeltaRequest {
        context_id: ContextId,
        delta_id: [u8; 32],
    },
    /// Request peer's current DAG heads for catchup
    DagHeadsRequest {
        context_id: ContextId,
    },
    /// Request sender_key for a specific identity (for decrypting deltas from unknown authors)
    IdentityRequest {
        context_id: ContextId,
        identity: PublicKey,
    },
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub enum MessagePayload<'a> {
    BlobShare {
        chunk: Cow<'a, [u8]>,
    },
    KeyShare {
        sender_key: PrivateKey,
    },
    /// Response to DeltaRequest containing the requested delta
    DeltaResponse {
        delta: Cow<'a, [u8]>,
    },
    /// Delta not found response
    DeltaNotFound,
    /// Response to DagHeadsRequest containing peer's current heads and root hash
    DagHeadsResponse {
        dag_heads: Vec<[u8; 32]>,
        root_hash: Hash,
    },
    /// Response to IdentityRequest containing the requested identity's sender_key
    IdentityResponse {
        identity: PublicKey,
        sender_key: Option<PrivateKey>,
    },
    /// Challenge to prove ownership of claimed identity
    Challenge {
        challenge: [u8; 32],
    },
    /// Response to challenge with signature (Ed25519 signature is 64 bytes)
    ChallengeResponse {
        signature: [u8; 64],
    },
}
