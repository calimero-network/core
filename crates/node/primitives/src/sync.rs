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
// Snapshot Sync Types
// =============================================================================

/// Request to negotiate a snapshot boundary for sync.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SnapshotBoundaryRequest {
    /// Context being synchronized.
    pub context_id: ContextId,

    /// Optional hint for boundary timestamp (nanoseconds since epoch).
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
}

/// Request to stream snapshot pages.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SnapshotStreamRequest {
    /// Context being synchronized.
    pub context_id: ContextId,

    /// Boundary root hash from the negotiated boundary.
    pub boundary_root_hash: Hash,

    /// Maximum number of pages to send in a burst.
    pub page_limit: u16,

    /// Maximum uncompressed bytes per page.
    pub byte_limit: u32,

    /// Optional cursor to resume paging.
    pub resume_cursor: Option<Vec<u8>>,
}

/// A page of snapshot data.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SnapshotPage {
    /// Compressed payload (lz4).
    pub payload: Vec<u8>,
    /// Expected size after decompression.
    pub uncompressed_len: u32,
    /// Next cursor; `None` indicates completion.
    pub cursor: Option<Vec<u8>>,
    /// Total pages in this stream (estimate).
    pub page_count: u64,
    /// Pages sent so far.
    pub sent_count: u64,
}

/// Cursor for resuming snapshot pagination.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SnapshotCursor {
    /// Last key sent in canonical order.
    pub last_key: [u8; 32],
}

/// Errors that can occur during snapshot sync.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum SnapshotError {
    /// Peer's delta history is pruned; full snapshot required.
    SnapshotRequired,
    /// The requested boundary is invalid or no longer available.
    InvalidBoundary,
    /// Resume cursor is invalid or expired.
    ResumeCursorInvalid,
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

        /// Sync hints for proactive divergence detection.
        /// Adds ~40 bytes overhead but enables faster sync triggering.
        sync_hints: crate::sync_protocol::SyncHints,
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
    /// Request snapshot boundary negotiation.
    SnapshotBoundaryRequest {
        context_id: ContextId,
        requested_cutoff_timestamp: Option<u64>,
    },
    /// Request to stream snapshot pages.
    SnapshotStreamRequest {
        context_id: ContextId,
        boundary_root_hash: Hash,
        page_limit: u16,
        byte_limit: u32,
        resume_cursor: Option<Vec<u8>>,
    },
    /// Sync handshake for protocol negotiation.
    SyncHandshake {
        handshake: crate::sync_protocol::SyncHandshake,
    },
    /// Request tree node(s) for hash comparison sync.
    ///
    /// Used by HashComparison, SubtreePrefetch, and LevelWise strategies.
    TreeNodeRequest {
        context_id: ContextId,
        /// Node IDs to fetch (hash of the node key/path).
        /// Empty = request root node.
        node_ids: Vec<[u8; 32]>,
        /// Maximum depth to include children (0 = node only, 1 = immediate children, etc.)
        include_children_depth: u8,
    },
    /// Request using bloom filter for efficient diff detection.
    BloomFilterRequest {
        context_id: ContextId,
        /// Serialized bloom filter containing local entity IDs.
        bloom_filter: Vec<u8>,
        /// Expected false positive rate used to construct the filter.
        false_positive_rate: f32,
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
    /// Response to SnapshotBoundaryRequest
    SnapshotBoundaryResponse {
        /// Authoritative boundary timestamp (nanoseconds since epoch).
        boundary_timestamp: u64,
        /// Root hash for the boundary state.
        boundary_root_hash: Hash,
        /// Peer's DAG heads at the boundary.
        dag_heads: Vec<[u8; 32]>,
    },
    /// A page of snapshot data.
    SnapshotPage {
        payload: Cow<'a, [u8]>,
        uncompressed_len: u32,
        cursor: Option<Vec<u8>>,
        page_count: u64,
        sent_count: u64,
    },
    /// Snapshot sync error.
    SnapshotError {
        error: SnapshotError,
    },
    /// Response to sync handshake with negotiated protocol.
    SyncHandshakeResponse {
        response: crate::sync_protocol::SyncHandshakeResponse,
    },
    /// Response to TreeNodeRequest containing tree node data.
    TreeNodeResponse {
        /// Requested nodes with their data.
        nodes: Vec<TreeNode>,
    },
    /// Response to BloomFilterRequest containing entities missing from requester.
    BloomFilterResponse {
        /// Entities that were NOT in the requester's bloom filter.
        /// Each entry includes key, value, AND metadata for proper CRDT merge.
        missing_entities: Vec<TreeLeafData>,
        /// Count of entities that matched the filter (for diagnostics).
        matched_count: u32,
    },
}

// =============================================================================
// Tree Sync Types (for HashComparison, SubtreePrefetch, LevelWise)
// =============================================================================

/// A tree node for hash comparison sync.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct TreeNode {
    /// Node ID (hash of the key/path).
    pub node_id: [u8; 32],
    /// Node's hash (for comparison).
    pub hash: Hash,
    /// If this is a leaf node, contains the entity data with metadata.
    /// Includes key, value, and CRDT metadata for proper merge.
    pub leaf_data: Option<TreeLeafData>,
    /// Child node IDs and hashes (for internal nodes).
    pub children: Vec<TreeNodeChild>,
}

/// Leaf entity data including metadata for CRDT merge.
///
/// This is sent over the wire during tree sync so the receiving node
/// has the `crdt_type` needed to perform proper CRDT merge.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct TreeLeafData {
    /// Entity key (32 bytes).
    pub key: [u8; 32],
    /// Entity value (serialized data).
    pub value: Vec<u8>,
    /// Entity metadata including crdt_type for merge dispatch.
    pub metadata: calimero_storage::entities::Metadata,
}

/// Reference to a child tree node.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct TreeNodeChild {
    /// Child node ID.
    pub node_id: [u8; 32],
    /// Child node's hash.
    pub hash: Hash,
}
