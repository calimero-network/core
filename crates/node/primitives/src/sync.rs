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
// Sync Handshake Protocol Types (CIP §2 - Sync Handshake Protocol)
// =============================================================================

/// Wire protocol version for sync handshake.
///
/// Increment on breaking changes to ensure nodes can detect incompatibility.
pub const SYNC_PROTOCOL_VERSION: u32 = 1;

/// Sync protocol selection for negotiation.
///
/// Each variant represents a different synchronization strategy with different
/// trade-offs in terms of bandwidth, latency, and computational overhead.
///
/// See CIP §1 - Sync Protocol Types.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub enum SyncProtocol {
    /// No sync needed - root hashes already match.
    None,

    /// Delta-based sync via DAG traversal.
    ///
    /// Best for: Small gaps, real-time updates.
    DeltaSync {
        /// Delta IDs that the requester is missing.
        missing_delta_ids: Vec<[u8; 32]>,
    },

    /// Hash-based Merkle tree comparison.
    ///
    /// Best for: General-purpose catch-up, 10-50% divergence.
    HashComparison {
        /// Root hash to compare against.
        root_hash: [u8; 32],
        /// Subtree roots that differ (if known).
        divergent_subtrees: Vec<[u8; 32]>,
    },

    /// Full state snapshot transfer.
    ///
    /// **CRITICAL**: Only valid for fresh nodes (Invariant I5).
    /// Initialized nodes MUST use state-based sync with CRDT merge instead.
    Snapshot {
        /// Whether the snapshot is compressed.
        compressed: bool,
        /// Whether the responder guarantees snapshot is verifiable.
        verified: bool,
    },

    /// Bloom filter-based quick diff.
    ///
    /// Best for: Large trees with small diff (<10% divergence).
    BloomFilter {
        /// Size of the bloom filter in bits.
        filter_size: usize,
        /// Expected false positive rate (0.0 to 1.0).
        false_positive_rate: f64,
    },

    /// Subtree prefetch for deep localized changes.
    ///
    /// Best for: Deep hierarchies with localized changes.
    SubtreePrefetch {
        /// Root IDs of subtrees to prefetch.
        subtree_roots: Vec<[u8; 32]>,
    },

    /// Level-wise sync for wide shallow trees.
    ///
    /// Best for: Trees with depth ≤ 2 and many children.
    LevelWise {
        /// Maximum depth to sync.
        max_depth: usize,
    },
}

impl Default for SyncProtocol {
    fn default() -> Self {
        Self::None
    }
}

/// Capabilities advertised during sync negotiation.
///
/// Used to determine mutually supported features between peers.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SyncCapabilities {
    /// Whether compression is supported.
    pub supports_compression: bool,
    /// Maximum entities per batch transfer.
    pub max_batch_size: usize,
    /// Protocols this node supports (ordered by preference).
    pub supported_protocols: Vec<SyncProtocol>,
}

impl Default for SyncCapabilities {
    fn default() -> Self {
        Self {
            supports_compression: true,
            max_batch_size: 1000,
            supported_protocols: vec![
                SyncProtocol::None,
                SyncProtocol::DeltaSync {
                    missing_delta_ids: vec![],
                },
                SyncProtocol::HashComparison {
                    root_hash: [0; 32],
                    divergent_subtrees: vec![],
                },
                SyncProtocol::Snapshot {
                    compressed: true,
                    verified: true,
                },
            ],
        }
    }
}

/// Sync handshake message (Initiator → Responder).
///
/// Contains the initiator's state summary for protocol negotiation.
///
/// See CIP §2.1 - Handshake Message.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SyncHandshake {
    /// Protocol version for compatibility checking.
    pub version: u32,
    /// Current Merkle root hash.
    pub root_hash: [u8; 32],
    /// Number of entities in the tree.
    pub entity_count: usize,
    /// Maximum depth of the Merkle tree.
    pub max_depth: usize,
    /// Current DAG heads (latest delta IDs).
    pub dag_heads: Vec<[u8; 32]>,
    /// Whether this node has any state.
    pub has_state: bool,
    /// Supported protocols (ordered by preference).
    pub supported_protocols: Vec<SyncProtocol>,
}

impl SyncHandshake {
    /// Create a new handshake message from local state.
    #[must_use]
    pub fn new(
        root_hash: [u8; 32],
        entity_count: usize,
        max_depth: usize,
        dag_heads: Vec<[u8; 32]>,
    ) -> Self {
        let has_state = root_hash != [0; 32];
        Self {
            version: SYNC_PROTOCOL_VERSION,
            root_hash,
            entity_count,
            max_depth,
            dag_heads,
            has_state,
            supported_protocols: SyncCapabilities::default().supported_protocols,
        }
    }

    /// Check if the remote handshake has a compatible protocol version.
    #[must_use]
    pub fn is_version_compatible(&self, other: &Self) -> bool {
        self.version == other.version
    }

    /// Check if root hashes match (already in sync).
    #[must_use]
    pub fn is_in_sync(&self, other: &Self) -> bool {
        self.root_hash == other.root_hash
    }
}

impl Default for SyncHandshake {
    fn default() -> Self {
        Self::new([0; 32], 0, 0, vec![])
    }
}

/// Sync handshake response (Responder → Initiator).
///
/// Contains the selected protocol and responder's state summary.
///
/// See CIP §2.2 - Negotiation Flow.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SyncHandshakeResponse {
    /// Protocol selected for this sync session.
    pub selected_protocol: SyncProtocol,
    /// Responder's current root hash.
    pub root_hash: [u8; 32],
    /// Responder's entity count.
    pub entity_count: usize,
    /// Responder's capabilities.
    pub capabilities: SyncCapabilities,
}

impl SyncHandshakeResponse {
    /// Create a response indicating no sync is needed.
    #[must_use]
    pub fn already_synced(root_hash: [u8; 32], entity_count: usize) -> Self {
        Self {
            selected_protocol: SyncProtocol::None,
            root_hash,
            entity_count,
            capabilities: SyncCapabilities::default(),
        }
    }

    /// Create a response with a selected protocol.
    #[must_use]
    pub fn with_protocol(
        selected_protocol: SyncProtocol,
        root_hash: [u8; 32],
        entity_count: usize,
    ) -> Self {
        Self {
            selected_protocol,
            root_hash,
            entity_count,
            capabilities: SyncCapabilities::default(),
        }
    }
}

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
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_protocol_roundtrip() {
        let protocols = vec![
            SyncProtocol::None,
            SyncProtocol::DeltaSync {
                missing_delta_ids: vec![[1; 32], [2; 32]],
            },
            SyncProtocol::HashComparison {
                root_hash: [3; 32],
                divergent_subtrees: vec![[4; 32]],
            },
            SyncProtocol::Snapshot {
                compressed: true,
                verified: false,
            },
            SyncProtocol::BloomFilter {
                filter_size: 1024,
                false_positive_rate: 0.01,
            },
            SyncProtocol::SubtreePrefetch {
                subtree_roots: vec![[5; 32], [6; 32]],
            },
            SyncProtocol::LevelWise { max_depth: 3 },
        ];

        for protocol in protocols {
            let encoded = borsh::to_vec(&protocol).expect("serialize");
            let decoded: SyncProtocol = borsh::from_slice(&encoded).expect("deserialize");
            assert_eq!(protocol, decoded);
        }
    }

    #[test]
    fn test_sync_capabilities_roundtrip() {
        let caps = SyncCapabilities::default();
        let encoded = borsh::to_vec(&caps).expect("serialize");
        let decoded: SyncCapabilities = borsh::from_slice(&encoded).expect("deserialize");
        assert_eq!(caps, decoded);
    }

    #[test]
    fn test_sync_handshake_roundtrip() {
        let handshake = SyncHandshake::new([42; 32], 100, 5, vec![[1; 32], [2; 32]]);

        let encoded = borsh::to_vec(&handshake).expect("serialize");
        let decoded: SyncHandshake = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(handshake, decoded);
        assert_eq!(decoded.version, SYNC_PROTOCOL_VERSION);
        assert!(decoded.has_state); // non-zero root_hash
    }

    #[test]
    fn test_sync_handshake_response_roundtrip() {
        let response = SyncHandshakeResponse::with_protocol(
            SyncProtocol::HashComparison {
                root_hash: [7; 32],
                divergent_subtrees: vec![],
            },
            [8; 32],
            500,
        );

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: SyncHandshakeResponse = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(response, decoded);
    }

    #[test]
    fn test_sync_handshake_version_compatibility() {
        let local = SyncHandshake::new([1; 32], 10, 2, vec![]);
        let compatible = SyncHandshake::new([2; 32], 20, 3, vec![]);
        let incompatible = SyncHandshake {
            version: SYNC_PROTOCOL_VERSION + 1,
            ..SyncHandshake::default()
        };

        assert!(local.is_version_compatible(&compatible));
        assert!(!local.is_version_compatible(&incompatible));
    }

    #[test]
    fn test_sync_handshake_in_sync_detection() {
        let local = SyncHandshake::new([42; 32], 100, 5, vec![]);
        let same_hash = SyncHandshake::new([42; 32], 200, 6, vec![[1; 32]]);
        let different_hash = SyncHandshake::new([99; 32], 100, 5, vec![]);

        assert!(local.is_in_sync(&same_hash));
        assert!(!local.is_in_sync(&different_hash));
    }

    #[test]
    fn test_sync_handshake_fresh_node() {
        let fresh = SyncHandshake::new([0; 32], 0, 0, vec![]);
        assert!(!fresh.has_state);

        let initialized = SyncHandshake::new([1; 32], 1, 1, vec![]);
        assert!(initialized.has_state);
    }

    #[test]
    fn test_sync_handshake_response_already_synced() {
        let response = SyncHandshakeResponse::already_synced([42; 32], 100);
        assert_eq!(response.selected_protocol, SyncProtocol::None);
    }
}
