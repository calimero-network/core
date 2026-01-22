#![expect(single_use_lifetimes, reason = "borsh shenanigans")]

use std::borrow::Cow;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_crypto::Nonce;
use calimero_network_primitives::specialized_node_invite::SpecializedNodeType;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};

// =============================================================================
// Snapshot Sync Types (Phase 1)
// =============================================================================

/// Request to negotiate a snapshot boundary for sync.
///
/// The responder will choose a boundary (typically current state) and return
/// metadata needed for the requester to decide whether to proceed with
/// snapshot sync or use an alternative sync method.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SnapshotBoundaryRequest {
    /// Context being synchronized.
    pub context_id: ContextId,

    /// Optional hint for boundary timestamp (nanoseconds since epoch).
    /// Responder may ignore this in Phase 1 and always use current state.
    pub requested_cutoff_timestamp: Option<u64>,
}

/// Response to snapshot boundary negotiation.
///
/// Contains the authoritative boundary state that the responder will serve
/// for the duration of this sync session.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SnapshotBoundaryResponse {
    /// Authoritative boundary timestamp (nanoseconds since epoch).
    pub boundary_timestamp: u64,

    /// Root hash for the boundary state; must be verified after apply.
    pub boundary_root_hash: Hash,

    /// Peer's DAG heads at the boundary; used for fine-sync after snapshot.
    pub dag_heads: Vec<[u8; 32]>,

    /// Optional uncompressed size estimate for progress reporting.
    pub total_estimate: Option<u64>,

    /// Optional Merkle tree parameters; presence enables Phase 2.
    /// None in Phase 1.
    pub tree_params: Option<TreeParams>,

    /// Optional total leaf count for progress/UI.
    pub leaf_count: Option<u64>,

    /// Optional Merkle root for the boundary (Phase 2).
    pub merkle_root_hash: Option<Hash>,
}

/// Request to stream snapshot pages.
///
/// The requester must first negotiate a boundary via `SnapshotBoundaryRequest`,
/// then use this request to stream the actual snapshot data.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SnapshotStreamRequest {
    /// Context being synchronized.
    pub context_id: ContextId,

    /// Boundary root hash from the negotiated boundary.
    /// Must match what the responder returned.
    pub boundary_root_hash: Hash,

    /// Maximum number of pages to send in a burst.
    pub page_limit: u16,

    /// Maximum uncompressed bytes per page (plus framing/overhead).
    pub byte_limit: u32,

    /// Optional cursor to resume paging from a previous session.
    pub resume_cursor: Option<Vec<u8>>,
}

/// A page of snapshot data.
///
/// Contains a compressed chunk of the canonical snapshot stream.
/// Pages are sent in order until `cursor` is `None` (completion).
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SnapshotPage {
    /// Compressed payload (lz4_flex with size prefix).
    pub payload: Vec<u8>,

    /// Expected size after decompression.
    pub uncompressed_len: u32,

    /// Next cursor for pagination; `None` or empty indicates completion.
    pub cursor: Option<Vec<u8>>,

    /// Total number of pages in this stream session (best effort estimate).
    pub page_count: u64,

    /// Cumulative pages sent so far.
    pub sent_count: u64,
}

/// Cursor for resuming snapshot pagination.
///
/// Serialized with Borsh and passed opaquely between requests.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SnapshotCursor {
    /// Last key hash sent in canonical order.
    pub last_key: [u8; 32],
}

/// Errors that can occur during snapshot sync.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum SnapshotError {
    /// Peer's delta history is pruned; full snapshot required.
    /// Used by the delta sync path to signal fallback.
    SnapshotRequired,

    /// The requested boundary is invalid or no longer available.
    InvalidBoundary,

    /// Requested page size exceeds maximum allowed.
    PageTooLarge,

    /// Snapshot sync not supported by this peer.
    Unsupported,

    /// Resume cursor is invalid or expired.
    ResumeCursorInvalid,
}

// =============================================================================
// Merkle Sync Types (Phase 2 - Placeholder)
// =============================================================================

/// Merkle tree parameters for Phase 2 sync.
///
/// These parameters must match between peers for Merkle sync to work.
/// If they don't match, fall back to snapshot sync.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TreeParams {
    /// Protocol schema version for tree params.
    pub version: u8,

    /// Hash algorithm used for leaf/internal nodes.
    pub hash_alg: HashAlg,

    /// Number of children per internal node.
    pub fanout: u16,

    /// Target uncompressed chunk size for leaves.
    pub leaf_target_bytes: u32,

    /// Snapshot encoding version to ensure deterministic bytes.
    pub encoding_version: u16,

    /// Defines canonical ordering and chunk split rules.
    pub chunking: ChunkingSpec,
}

/// Hash algorithm for Merkle tree nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum HashAlg {
    /// SHA-256 truncated to 256 bits (full hash).
    Sha256_256 = 1,
}

/// Specification for how snapshot data is chunked.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum ChunkingSpec {
    /// Sort entries by canonical key ordering before chunking.
    BySortedKeys {
        /// Encoding version for keys.
        key_encoding: u16,
        /// Encoding version for values.
        value_encoding: u16,
        /// Whether to include index entries.
        include_indexes: bool,
        /// Whether to include entity entries.
        include_entries: bool,
    },
}

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

    /// Specialized node discovery request
    ///
    /// Broadcast by a node to discover and invite specialized nodes (e.g., read-only TEE nodes).
    /// Specialized nodes receiving this will respond via request-response protocol
    /// to the message source (available from gossipsub message).
    ///
    /// Note: context_id is NOT included - it's tracked internally by the requesting
    /// node using the nonce as the lookup key.
    SpecializedNodeDiscovery {
        /// Random nonce to bind verification to this request
        nonce: [u8; 32],
        /// Type of specialized node being invited
        node_type: SpecializedNodeType,
    },

    /// Confirmation that a specialized node has joined a context
    ///
    /// Broadcast by specialized nodes on the context topic after successfully joining.
    /// The inviting node receives this and removes the pending invite entry.
    SpecializedNodeJoinConfirmation {
        /// The nonce from the original discovery request
        nonce: [u8; 32],
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

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
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
    /// Request snapshot boundary negotiation (Phase 1)
    SnapshotBoundaryRequest {
        /// Context being synchronized.
        context_id: ContextId,
        /// Optional hint for boundary timestamp (nanoseconds since epoch).
        requested_cutoff_timestamp: Option<u64>,
    },
    /// Request to stream snapshot pages (Phase 1)
    SnapshotStreamRequest {
        /// Context being synchronized.
        context_id: ContextId,
        /// Boundary root hash from negotiated boundary.
        boundary_root_hash: Hash,
        /// Maximum number of pages to send in a burst.
        page_limit: u16,
        /// Maximum uncompressed bytes per page.
        byte_limit: u32,
        /// Optional cursor to resume paging.
        resume_cursor: Option<Vec<u8>>,
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
    /// Challenge to prove ownership of claimed identity
    Challenge {
        challenge: [u8; 32],
    },
    /// Response to challenge with signature (Ed25519 signature is 64 bytes)
    ChallengeResponse {
        signature: [u8; 64],
    },
    /// Response to SnapshotBoundaryRequest (Phase 1)
    SnapshotBoundaryResponse {
        /// Authoritative boundary timestamp (nanoseconds since epoch).
        boundary_timestamp: u64,
        /// Root hash for the boundary state.
        boundary_root_hash: Hash,
        /// Peer's DAG heads at the boundary.
        dag_heads: Vec<[u8; 32]>,
        /// Optional uncompressed size estimate for progress.
        total_estimate: Option<u64>,
        /// Optional Merkle parameters (Phase 2).
        tree_params: Option<TreeParams>,
        /// Optional total leaf count.
        leaf_count: Option<u64>,
        /// Optional Merkle root (Phase 2).
        merkle_root_hash: Option<Hash>,
    },
    /// A page of snapshot data (Phase 1)
    SnapshotPage {
        /// Compressed payload (lz4_flex with size prefix).
        payload: Cow<'a, [u8]>,
        /// Expected size after decompression.
        uncompressed_len: u32,
        /// Next cursor; None indicates completion.
        cursor: Option<Vec<u8>>,
        /// Total pages estimate.
        page_count: u64,
        /// Cumulative pages sent.
        sent_count: u64,
    },
    /// Snapshot sync error (Phase 1)
    SnapshotError {
        error: SnapshotError,
    },
}
