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

// =============================================================================
// Merkle Sync Types (Phase 2)
// =============================================================================

/// Hash algorithm used for Merkle tree nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum HashAlg {
    /// SHA-256 with 256-bit output.
    Sha256 = 1,
}

/// Specification for how snapshot data is chunked into Merkle leaves.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum ChunkingSpec {
    /// Chunk by sorted keys with specified encoding versions.
    BySortedKeys {
        /// Encoding version for keys.
        key_encoding: u16,
        /// Encoding version for values.
        value_encoding: u16,
        /// Whether to include index entries.
        include_indexes: bool,
        /// Whether to include data entries.
        include_entries: bool,
    },
}

/// Parameters defining the Merkle tree structure.
///
/// Both peers must agree on these parameters for Merkle sync to work.
/// If parameters don't match, fall back to snapshot sync.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TreeParams {
    /// Protocol schema version for tree params.
    pub version: u8,
    /// Hash algorithm used for leaf and internal nodes.
    pub hash_alg: HashAlg,
    /// Number of children per internal node.
    pub fanout: u16,
    /// Target uncompressed chunk size for leaves (bytes).
    pub leaf_target_bytes: u32,
    /// Snapshot encoding version to ensure deterministic bytes.
    pub encoding_version: u16,
    /// Defines canonical ordering and chunk split rules.
    pub chunking: ChunkingSpec,
}

impl Default for TreeParams {
    fn default() -> Self {
        Self {
            version: 1,
            hash_alg: HashAlg::Sha256,
            fanout: 16,
            leaf_target_bytes: 64 * 1024, // 64 KiB, matches DEFAULT_PAGE_BYTE_LIMIT
            encoding_version: 1,
            chunking: ChunkingSpec::BySortedKeys {
                key_encoding: 1,
                value_encoding: 1,
                include_indexes: true,
                include_entries: true,
            },
        }
    }
}

impl TreeParams {
    /// Check if these params are compatible with another set.
    ///
    /// Parameters must match exactly for Merkle sync to work.
    pub fn is_compatible(&self, other: &Self) -> bool {
        self == other
    }
}

/// A chunk of snapshot data for Merkle sync.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SnapshotChunk {
    /// Deterministic leaf index in the ordered stream.
    pub index: u64,
    /// First key in this chunk (inclusive).
    pub start_key: Vec<u8>,
    /// Last key in this chunk (inclusive).
    pub end_key: Vec<u8>,
    /// Expected payload size after decompression.
    pub uncompressed_len: u32,
    /// Raw (uncompressed) canonical bytes.
    pub payload: Vec<u8>,
}

/// Identifies a node in the Merkle tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize)]
pub struct NodeId {
    /// Tree level (0 = leaves).
    pub level: u16,
    /// Node index within its level.
    pub index: u64,
}

/// Hash digest for a Merkle tree node.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct NodeDigest {
    /// Node identifier.
    pub id: NodeId,
    /// Hash for this node (leaf or internal).
    pub hash: Hash,
    /// Number of children (for internal nodes) or 0 for leaves.
    pub child_count: u16,
}

/// Cursor for resuming Merkle sync traversal.
///
/// Maximum serialized size is capped at 64 KiB. If exceeded,
/// fall back to snapshot sync.
#[derive(Clone, Debug, Default, BorshSerialize, BorshDeserialize)]
pub struct MerkleCursor {
    /// Pending internal nodes to request hashes for.
    pub pending_nodes: Vec<NodeId>,
    /// Pending leaf indices to request chunks for.
    pub pending_leaves: Vec<u64>,
}

/// Maximum serialized size for MerkleCursor (64 KiB).
pub const MERKLE_CURSOR_MAX_SIZE: usize = 64 * 1024;

impl MerkleCursor {
    /// Check if the cursor would exceed the size limit when serialized.
    pub fn exceeds_size_limit(&self) -> bool {
        // Estimate: NodeId is ~10 bytes, u64 is 8 bytes
        let estimated_size = self.pending_nodes.len() * 10 + self.pending_leaves.len() * 8 + 16;
        estimated_size > MERKLE_CURSOR_MAX_SIZE
    }
}

/// Request to start a Merkle sync session.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct MerkleSyncRequest {
    /// Context being synchronized.
    pub context_id: ContextId,
    /// Boundary to sync against; must match snapshot boundary.
    pub boundary_root_hash: Hash,
    /// Merkle parameters; must match responder.
    pub tree_params: TreeParams,
    /// Maximum nodes/leaves per reply.
    pub page_limit: u16,
    /// Maximum uncompressed bytes per leaf reply batch.
    pub byte_limit: u32,
    /// Resume position in traversal (serialized MerkleCursor).
    pub resume_cursor: Option<Vec<u8>>,
    /// Requester's current Merkle root (optional optimization).
    pub requester_root_hash: Option<Hash>,
}

/// Frame types for Merkle sync protocol.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum MerkleSyncFrame {
    /// Requester asks for node hashes.
    NodeRequest { nodes: Vec<NodeId> },
    /// Responder returns node hashes.
    NodeReply { nodes: Vec<NodeDigest> },
    /// Requester asks for leaf payloads by index.
    LeafRequest { leaves: Vec<u64> },
    /// Responder returns leaf chunks.
    LeafReply { leaves: Vec<SnapshotChunk> },
    /// Requester signals traversal complete.
    Done,
    /// Protocol error with code and message.
    Error { code: u16, message: String },
}

/// Error codes for MerkleSyncFrame::Error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum MerkleErrorCode {
    /// The requested boundary is invalid or no longer available.
    InvalidBoundary = 1,
    /// Merkle sync is not supported.
    Unsupported = 2,
    /// Requested page/batch is too large.
    PageTooLarge = 3,
    /// Tree parameters are incompatible.
    IncompatibleParams = 4,
    /// Verification of Merkle root failed.
    VerificationFailed = 5,
    /// Resume cursor is invalid or too large.
    ResumeCursorInvalid = 6,
}

impl MerkleErrorCode {
    pub fn as_u16(self) -> u16 {
        self as u16
    }

    pub fn from_u16(code: u16) -> Option<Self> {
        match code {
            1 => Some(Self::InvalidBoundary),
            2 => Some(Self::Unsupported),
            3 => Some(Self::PageTooLarge),
            4 => Some(Self::IncompatibleParams),
            5 => Some(Self::VerificationFailed),
            6 => Some(Self::ResumeCursorInvalid),
            _ => None,
        }
    }
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
    /// Request to start a Merkle sync session.
    MerkleSyncRequest {
        context_id: ContextId,
        boundary_root_hash: Hash,
        tree_params: TreeParams,
        page_limit: u16,
        byte_limit: u32,
        resume_cursor: Option<Vec<u8>>,
        requester_root_hash: Option<Hash>,
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
        /// Optional Merkle tree parameters; presence enables Phase 2 Merkle sync.
        tree_params: Option<TreeParams>,
        /// Optional total leaf count for progress/UI.
        leaf_count: Option<u64>,
        /// Optional Merkle root hash for the boundary state.
        merkle_root_hash: Option<Hash>,
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
    /// Merkle sync frame (for bidirectional Merkle protocol messages).
    MerkleSyncFrame {
        frame: MerkleSyncFrame,
    },
}
