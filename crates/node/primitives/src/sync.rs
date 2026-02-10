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

/// Protocol capability identifier for sync negotiation.
///
/// This is a discriminant-only enum used for advertising which sync protocols
/// a node supports. Unlike [`SyncProtocol`], this does not carry protocol-specific
/// data, making it suitable for capability comparison with `contains()` and equality.
///
/// See CIP §2 - Sync Handshake Protocol.
///
/// **IMPORTANT**: Keep variants in sync with [`SyncProtocol`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize)]
pub enum SyncProtocolKind {
    /// No sync needed - root hashes already match.
    None,
    /// Delta-based sync via DAG traversal.
    DeltaSync,
    /// Hash-based Merkle tree comparison.
    HashComparison,
    /// Full state snapshot transfer.
    Snapshot,
    /// Bloom filter-based quick diff.
    BloomFilter,
    /// Subtree prefetch for deep localized changes.
    SubtreePrefetch,
    /// Level-wise sync for wide shallow trees.
    LevelWise,
}

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
        filter_size: u64,
        /// Expected false positive rate (0.0 to 1.0).
        ///
        /// **Note**: Validation of bounds is performed when constructing the actual
        /// bloom filter, not at protocol negotiation time. Invalid values will cause
        /// filter construction to fail gracefully.
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
        max_depth: u32,
    },
}

impl Default for SyncProtocol {
    fn default() -> Self {
        Self::None
    }
}

impl SyncProtocol {
    /// Returns the protocol kind (discriminant) for this protocol.
    ///
    /// Useful for capability matching where the protocol-specific data is irrelevant.
    #[must_use]
    pub fn kind(&self) -> SyncProtocolKind {
        SyncProtocolKind::from(self)
    }
}

impl From<&SyncProtocol> for SyncProtocolKind {
    fn from(protocol: &SyncProtocol) -> Self {
        match protocol {
            SyncProtocol::None => Self::None,
            SyncProtocol::DeltaSync { .. } => Self::DeltaSync,
            SyncProtocol::HashComparison { .. } => Self::HashComparison,
            SyncProtocol::Snapshot { .. } => Self::Snapshot,
            SyncProtocol::BloomFilter { .. } => Self::BloomFilter,
            SyncProtocol::SubtreePrefetch { .. } => Self::SubtreePrefetch,
            SyncProtocol::LevelWise { .. } => Self::LevelWise,
        }
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
    pub max_batch_size: u64,
    /// Protocols this node supports (ordered by preference).
    pub supported_protocols: Vec<SyncProtocolKind>,
}

impl Default for SyncCapabilities {
    fn default() -> Self {
        Self {
            supports_compression: true,
            max_batch_size: 1000,
            supported_protocols: vec![
                SyncProtocolKind::None,
                SyncProtocolKind::DeltaSync,
                SyncProtocolKind::HashComparison,
                SyncProtocolKind::Snapshot,
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
    pub entity_count: u64,
    /// Maximum depth of the Merkle tree.
    pub max_depth: u32,
    /// Current DAG heads (latest delta IDs).
    pub dag_heads: Vec<[u8; 32]>,
    /// Whether this node has any state.
    pub has_state: bool,
    /// Supported protocols (ordered by preference).
    pub supported_protocols: Vec<SyncProtocolKind>,
}

impl SyncHandshake {
    /// Create a new handshake message from local state.
    #[must_use]
    pub fn new(
        root_hash: [u8; 32],
        entity_count: u64,
        max_depth: u32,
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
    pub entity_count: u64,
    /// Responder's capabilities.
    pub capabilities: SyncCapabilities,
}

impl SyncHandshakeResponse {
    /// Create a response indicating no sync is needed.
    #[must_use]
    pub fn already_synced(root_hash: [u8; 32], entity_count: u64) -> Self {
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
        entity_count: u64,
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
// Protocol Selection (CIP §2.3 - Protocol Selection Rules)
// =============================================================================

/// Result of protocol selection with reasoning.
#[derive(Clone, Debug)]
pub struct ProtocolSelection {
    /// The selected protocol.
    pub protocol: SyncProtocol,
    /// Human-readable reason for the selection (for logging).
    pub reason: &'static str,
}

/// Calculate the divergence ratio between two handshakes.
///
/// Formula: `|local.entity_count - remote.entity_count| / max(remote.entity_count, 1)`
///
/// Returns a value in [0.0, ∞), where:
/// - 0.0 = identical entity counts
/// - 1.0 = 100% divergence (e.g., local=0, remote=100)
/// - >1.0 = local has more entities than remote
#[must_use]
pub fn calculate_divergence(local: &SyncHandshake, remote: &SyncHandshake) -> f64 {
    // Use abs_diff to avoid overflow when entity_count exceeds i64::MAX
    let diff = local.entity_count.abs_diff(remote.entity_count);
    let denominator = remote.entity_count.max(1);
    diff as f64 / denominator as f64
}

/// Select the optimal sync protocol based on handshake information.
///
/// Implements the decision table from CIP §2.3:
///
/// | # | Condition | Selected Protocol |
/// |---|-----------|-------------------|
/// | 1 | `root_hash` match | `None` |
/// | 2 | `!has_state` (fresh node) | `Snapshot` |
/// | 3 | `has_state` AND divergence >50% | `HashComparison` |
/// | 4 | `max_depth` >3 AND divergence <20% | `SubtreePrefetch` |
/// | 5 | `entity_count` >50 AND divergence <10% | `BloomFilter` |
/// | 6 | `max_depth` 1-2 AND avg children/level >10 | `LevelWise` |
/// | 7 | (default) | `HashComparison` |
///
/// **CRITICAL (Invariant I5)**: Snapshot is NEVER selected for initialized nodes.
/// This prevents silent data loss from overwriting local CRDT state.
#[must_use]
pub fn select_protocol(local: &SyncHandshake, remote: &SyncHandshake) -> ProtocolSelection {
    // Rule 1: Already synced - no action needed
    if local.root_hash == remote.root_hash {
        return ProtocolSelection {
            protocol: SyncProtocol::None,
            reason: "root hashes match, already in sync",
        };
    }

    // Check version compatibility first
    if !local.is_version_compatible(remote) {
        // Version mismatch - fall back to HashComparison as safest option
        return ProtocolSelection {
            protocol: SyncProtocol::HashComparison {
                root_hash: remote.root_hash,
                divergent_subtrees: vec![],
            },
            reason: "version mismatch, using safe fallback",
        };
    }

    // Rule 2: Fresh node - Snapshot allowed
    // CRITICAL: This is the ONLY case where Snapshot is permitted
    if !local.has_state {
        return ProtocolSelection {
            protocol: SyncProtocol::Snapshot {
                compressed: remote.entity_count > 100,
                verified: true,
            },
            reason: "fresh node bootstrap via snapshot",
        };
    }

    // From here on, local HAS state - NEVER use Snapshot (Invariant I5)
    let divergence = calculate_divergence(local, remote);

    // Rule 3: Large divergence (>50%) - use HashComparison with CRDT merge
    if divergence > 0.5 {
        return ProtocolSelection {
            protocol: SyncProtocol::HashComparison {
                root_hash: remote.root_hash,
                divergent_subtrees: vec![],
            },
            reason: "high divergence (>50%), using hash comparison with CRDT merge",
        };
    }

    // Rule 4: Deep tree with localized changes
    if remote.max_depth > 3 && divergence < 0.2 {
        return ProtocolSelection {
            protocol: SyncProtocol::SubtreePrefetch {
                subtree_roots: vec![], // Will be populated during sync
            },
            reason: "deep tree with low divergence, using subtree prefetch",
        };
    }

    // Rule 5: Large tree with small diff
    if remote.entity_count > 50 && divergence < 0.1 {
        return ProtocolSelection {
            protocol: SyncProtocol::BloomFilter {
                // ~10 bits per entity, max 10k; saturating to avoid overflow on huge counts
                filter_size: remote.entity_count.saturating_mul(10).min(10_000),
                false_positive_rate: 0.01,
            },
            reason: "large tree with small divergence, using bloom filter",
        };
    }

    // Rule 6: Wide shallow tree (depth 1-2 with many children per level)
    // Skip depth=0 (no hierarchy) - LevelWise requires actual tree structure
    if remote.max_depth >= 1 && remote.max_depth <= 2 {
        let avg_children_per_level = remote.entity_count / u64::from(remote.max_depth);
        if avg_children_per_level > 10 {
            return ProtocolSelection {
                protocol: SyncProtocol::LevelWise {
                    max_depth: remote.max_depth,
                },
                reason: "wide shallow tree, using level-wise sync",
            };
        }
    }

    // Rule 7: Default fallback
    ProtocolSelection {
        protocol: SyncProtocol::HashComparison {
            root_hash: remote.root_hash,
            divergent_subtrees: vec![],
        },
        reason: "default: using hash comparison",
    }
}

/// Check if a protocol kind is supported by the remote peer.
#[must_use]
pub fn is_protocol_supported(protocol: &SyncProtocol, capabilities: &SyncCapabilities) -> bool {
    capabilities.supported_protocols.contains(&protocol.kind())
}

/// Select protocol with fallback if preferred is not supported.
///
/// Tries the preferred protocol first, then falls back through the decision
/// table until a mutually supported protocol is found.
#[must_use]
pub fn select_protocol_with_fallback(
    local: &SyncHandshake,
    remote: &SyncHandshake,
    remote_capabilities: &SyncCapabilities,
) -> ProtocolSelection {
    let preferred = select_protocol(local, remote);

    // Check if preferred protocol is supported
    if is_protocol_supported(&preferred.protocol, remote_capabilities) {
        return preferred;
    }

    // Fallback: HashComparison is always safe for initialized nodes
    if local.has_state {
        let fallback = SyncProtocol::HashComparison {
            root_hash: remote.root_hash,
            divergent_subtrees: vec![],
        };
        if is_protocol_supported(&fallback, remote_capabilities) {
            return ProtocolSelection {
                protocol: fallback,
                reason: "fallback to hash comparison (preferred not supported)",
            };
        }
    }

    // Last resort: None (will need manual intervention)
    ProtocolSelection {
        protocol: SyncProtocol::None,
        reason: "no mutually supported protocol found",
    }
}

// =============================================================================
// Delta Sync Types (CIP §4 - State Machine, DELTA SYNC branch)
// =============================================================================

/// Default threshold for choosing delta sync vs state-based sync.
///
/// If fewer than this many deltas are missing, use delta sync.
/// If more are missing, escalate to state-based sync (HashComparison, etc.).
///
/// This is a heuristic balance between:
/// - Delta sync: O(missing) round trips, but exact
/// - State sync: O(log n) comparisons, but may transfer more data
pub const DEFAULT_DELTA_SYNC_THRESHOLD: usize = 150;

/// Request for delta-based synchronization.
///
/// Used when few deltas are missing and their IDs are known.
/// The responder should return the requested deltas in causal order.
///
/// See CIP §4 - State Machine (DELTA SYNC branch).
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct DeltaSyncRequest {
    /// IDs of missing deltas to request.
    pub missing_delta_ids: Vec<[u8; 32]>,
}

impl DeltaSyncRequest {
    /// Create a new delta sync request.
    #[must_use]
    pub fn new(missing_delta_ids: Vec<[u8; 32]>) -> Self {
        Self { missing_delta_ids }
    }

    /// Check if the request is within the recommended threshold.
    #[must_use]
    pub fn is_within_threshold(&self) -> bool {
        self.missing_delta_ids.len() <= DEFAULT_DELTA_SYNC_THRESHOLD
    }

    /// Number of deltas being requested.
    #[must_use]
    pub fn count(&self) -> usize {
        self.missing_delta_ids.len()
    }
}

/// Response to a delta sync request.
///
/// Contains the requested deltas in causal order (parents before children).
/// If some deltas are not found, they are omitted from the response.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct DeltaSyncResponse {
    /// Deltas in causal order (parents first).
    /// Each delta is serialized as bytes for transport.
    pub deltas: Vec<DeltaPayload>,

    /// IDs of deltas that were requested but not found.
    pub not_found: Vec<[u8; 32]>,
}

impl DeltaSyncResponse {
    /// Create a response with found deltas.
    #[must_use]
    pub fn new(deltas: Vec<DeltaPayload>, not_found: Vec<[u8; 32]>) -> Self {
        Self { deltas, not_found }
    }

    /// Create an empty response (no deltas found).
    #[must_use]
    pub fn empty(not_found: Vec<[u8; 32]>) -> Self {
        Self {
            deltas: vec![],
            not_found,
        }
    }

    /// Check if all requested deltas were found.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.not_found.is_empty()
    }

    /// Number of deltas returned.
    #[must_use]
    pub fn count(&self) -> usize {
        self.deltas.len()
    }
}

/// A delta payload for transport.
///
/// Contains the delta data and metadata needed for application.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct DeltaPayload {
    /// Unique delta ID (content hash).
    pub id: [u8; 32],

    /// Parent delta IDs (for causal ordering).
    pub parents: Vec<[u8; 32]>,

    /// Serialized delta operations (Borsh-encoded).
    pub payload: Vec<u8>,

    /// HLC timestamp when the delta was created.
    pub hlc_timestamp: u64,

    /// Expected root hash after applying this delta.
    ///
    /// This hash is captured by the originating node at delta creation time.
    /// It serves two purposes:
    /// 1. **Linear history**: When deltas are applied in sequence without
    ///    concurrent branches, receivers can verify they reach the same state.
    /// 2. **DAG consistency**: When concurrent deltas exist, this hash ensures
    ///    nodes build identical DAG structures even if their local root hashes
    ///    differ due to different merge ordering. The hash reflects the
    ///    originator's state, not a universal truth.
    ///
    /// **Verification strategy**: Compare against this hash only when applying
    /// deltas from a single linear chain. For concurrent/merged deltas, use
    /// the Merkle tree reconciliation protocol instead.
    pub expected_root_hash: [u8; 32],
}

impl DeltaPayload {
    /// Check if this delta has no parents (genesis delta).
    #[must_use]
    pub fn is_genesis(&self) -> bool {
        self.parents.is_empty()
    }
}

/// Result of attempting to apply deltas.
///
/// **Note**: This is a local-only type used to report the outcome of delta
/// application. It is not transmitted over the wire (hence no Borsh traits).
#[derive(Clone, Debug)]
pub enum DeltaApplyResult {
    /// All deltas applied successfully.
    Success {
        /// Number of deltas applied.
        applied_count: usize,
        /// New root hash after applying.
        new_root_hash: [u8; 32],
    },

    /// Some deltas could not be applied due to missing parents.
    /// Suggests escalation to state-based sync.
    MissingParents {
        /// Delta IDs whose parents are missing.
        missing_parent_deltas: Vec<[u8; 32]>,
        /// Number of deltas successfully applied before failure.
        applied_before_failure: usize,
    },

    /// Delta application failed (hash mismatch, corruption, etc.).
    Failed {
        /// Description of the failure.
        reason: String,
    },
}

impl DeltaApplyResult {
    /// Check if delta application was fully successful.
    #[must_use]
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    /// Check if escalation to state-based sync is needed.
    #[must_use]
    pub fn needs_state_sync(&self) -> bool {
        matches!(self, Self::MissingParents { .. })
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

    #[test]
    fn test_sync_protocol_kind_roundtrip() {
        let kinds = vec![
            SyncProtocolKind::None,
            SyncProtocolKind::DeltaSync,
            SyncProtocolKind::HashComparison,
            SyncProtocolKind::Snapshot,
            SyncProtocolKind::BloomFilter,
            SyncProtocolKind::SubtreePrefetch,
            SyncProtocolKind::LevelWise,
        ];

        for kind in kinds {
            let encoded = borsh::to_vec(&kind).expect("serialize");
            let decoded: SyncProtocolKind = borsh::from_slice(&encoded).expect("deserialize");
            assert_eq!(kind, decoded);
        }
    }

    #[test]
    fn test_sync_protocol_kind_conversion() {
        // Test kind() method and From trait
        assert_eq!(SyncProtocol::None.kind(), SyncProtocolKind::None);
        assert_eq!(
            SyncProtocol::DeltaSync {
                missing_delta_ids: vec![[1; 32]]
            }
            .kind(),
            SyncProtocolKind::DeltaSync
        );
        assert_eq!(
            SyncProtocol::HashComparison {
                root_hash: [2; 32],
                divergent_subtrees: vec![]
            }
            .kind(),
            SyncProtocolKind::HashComparison
        );
        assert_eq!(
            SyncProtocol::Snapshot {
                compressed: true,
                verified: true
            }
            .kind(),
            SyncProtocolKind::Snapshot
        );
        assert_eq!(
            SyncProtocol::BloomFilter {
                filter_size: 1024,
                false_positive_rate: 0.01
            }
            .kind(),
            SyncProtocolKind::BloomFilter
        );
        assert_eq!(
            SyncProtocol::SubtreePrefetch {
                subtree_roots: vec![]
            }
            .kind(),
            SyncProtocolKind::SubtreePrefetch
        );
        assert_eq!(
            SyncProtocol::LevelWise { max_depth: 5 }.kind(),
            SyncProtocolKind::LevelWise
        );

        // Test From trait directly
        let protocol = SyncProtocol::HashComparison {
            root_hash: [3; 32],
            divergent_subtrees: vec![],
        };
        let kind: SyncProtocolKind = (&protocol).into();
        assert_eq!(kind, SyncProtocolKind::HashComparison);
    }

    // =========================================================================
    // Protocol Selection Tests (Issue #1771)
    // =========================================================================

    #[test]
    fn test_calculate_divergence() {
        // Same counts
        let local = SyncHandshake::new([1; 32], 100, 5, vec![]);
        let remote = SyncHandshake::new([2; 32], 100, 5, vec![]);
        assert!((calculate_divergence(&local, &remote) - 0.0).abs() < f64::EPSILON);

        // 50% divergence
        let local = SyncHandshake::new([1; 32], 50, 5, vec![]);
        let remote = SyncHandshake::new([2; 32], 100, 5, vec![]);
        assert!((calculate_divergence(&local, &remote) - 0.5).abs() < f64::EPSILON);

        // 100% divergence (local empty)
        let local = SyncHandshake::new([1; 32], 0, 0, vec![]);
        let remote = SyncHandshake::new([2; 32], 100, 5, vec![]);
        assert!((calculate_divergence(&local, &remote) - 1.0).abs() < f64::EPSILON);

        // Handles zero remote count
        let local = SyncHandshake::new([1; 32], 100, 5, vec![]);
        let remote = SyncHandshake::new([2; 32], 0, 0, vec![]);
        assert!((calculate_divergence(&local, &remote) - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_select_protocol_rule1_already_synced() {
        let local = SyncHandshake::new([42; 32], 100, 5, vec![]);
        let remote = SyncHandshake::new([42; 32], 200, 3, vec![]); // Same root hash

        let selection = select_protocol(&local, &remote);
        assert!(matches!(selection.protocol, SyncProtocol::None));
        assert!(selection.reason.contains("already in sync"));
    }

    #[test]
    fn test_select_protocol_rule2_fresh_node_gets_snapshot() {
        let local = SyncHandshake::new([0; 32], 0, 0, vec![]); // Fresh node
        let remote = SyncHandshake::new([42; 32], 200, 5, vec![]);

        let selection = select_protocol(&local, &remote);
        assert!(matches!(selection.protocol, SyncProtocol::Snapshot { .. }));
        assert!(selection.reason.contains("fresh node"));
    }

    #[test]
    fn test_select_protocol_rule3_initialized_node_never_gets_snapshot() {
        // CRITICAL TEST for Invariant I5
        let local = SyncHandshake::new([1; 32], 1, 1, vec![]); // Has state!
        let remote = SyncHandshake::new([42; 32], 200, 5, vec![]);

        let selection = select_protocol(&local, &remote);
        // Even with high divergence, should NOT get Snapshot
        assert!(!matches!(selection.protocol, SyncProtocol::Snapshot { .. }));
    }

    #[test]
    fn test_select_protocol_rule3_high_divergence_uses_hash_comparison() {
        let local = SyncHandshake::new([1; 32], 10, 2, vec![]); // Has state
        let remote = SyncHandshake::new([2; 32], 100, 5, vec![]); // 90% divergence

        let selection = select_protocol(&local, &remote);
        assert!(matches!(
            selection.protocol,
            SyncProtocol::HashComparison { .. }
        ));
        assert!(selection.reason.contains("divergence"));
    }

    #[test]
    fn test_select_protocol_rule4_deep_tree_uses_subtree_prefetch() {
        let local = SyncHandshake::new([1; 32], 90, 5, vec![]); // ~10% divergence
        let remote = SyncHandshake::new([2; 32], 100, 5, vec![]); // depth > 3

        let selection = select_protocol(&local, &remote);
        assert!(matches!(
            selection.protocol,
            SyncProtocol::SubtreePrefetch { .. }
        ));
        assert!(selection.reason.contains("subtree"));
    }

    #[test]
    fn test_select_protocol_rule5_large_tree_small_diff_uses_bloom() {
        let local = SyncHandshake::new([1; 32], 95, 2, vec![]); // ~5% divergence
        let remote = SyncHandshake::new([2; 32], 100, 2, vec![]); // entity_count > 50

        let selection = select_protocol(&local, &remote);
        assert!(matches!(
            selection.protocol,
            SyncProtocol::BloomFilter { .. }
        ));
        assert!(selection.reason.contains("bloom"));
    }

    #[test]
    fn test_select_protocol_rule6_wide_shallow_uses_levelwise() {
        // Wide shallow tree: max_depth <= 2, many children per level, entity_count <= 50
        // (to avoid triggering bloom filter rule first)
        let local = SyncHandshake::new([1; 32], 40, 2, vec![]);
        let remote = SyncHandshake::new([2; 32], 40, 2, vec![]);

        let selection = select_protocol(&local, &remote);
        assert!(matches!(selection.protocol, SyncProtocol::LevelWise { .. }));
        assert!(selection.reason.contains("level"));
    }

    #[test]
    fn test_select_protocol_rule7_default_uses_hash_comparison() {
        // Create conditions that don't match any specific rule
        let local = SyncHandshake::new([1; 32], 30, 2, vec![]); // ~25% divergence
        let remote = SyncHandshake::new([2; 32], 40, 3, vec![]); // depth=3, not >3

        let selection = select_protocol(&local, &remote);
        assert!(matches!(
            selection.protocol,
            SyncProtocol::HashComparison { .. }
        ));
        assert!(selection.reason.contains("default"));
    }

    #[test]
    fn test_select_protocol_version_mismatch_uses_safe_fallback() {
        let local = SyncHandshake::new([1; 32], 100, 5, vec![]);
        let mut remote = SyncHandshake::new([2; 32], 100, 5, vec![]);
        remote.version = SYNC_PROTOCOL_VERSION + 1; // Incompatible version

        let selection = select_protocol(&local, &remote);
        assert!(matches!(
            selection.protocol,
            SyncProtocol::HashComparison { .. }
        ));
        assert!(selection.reason.contains("version mismatch"));
    }

    #[test]
    fn test_is_protocol_supported() {
        let caps = SyncCapabilities::default();

        // Supported (in default list)
        assert!(is_protocol_supported(&SyncProtocol::None, &caps));
        assert!(is_protocol_supported(
            &SyncProtocol::HashComparison {
                root_hash: [0; 32],
                divergent_subtrees: vec![]
            },
            &caps
        ));

        // Not supported (not in default list)
        assert!(!is_protocol_supported(
            &SyncProtocol::SubtreePrefetch {
                subtree_roots: vec![]
            },
            &caps
        ));
        assert!(!is_protocol_supported(
            &SyncProtocol::LevelWise { max_depth: 2 },
            &caps
        ));
    }

    #[test]
    fn test_select_protocol_with_fallback() {
        let local = SyncHandshake::new([1; 32], 90, 5, vec![]); // Would prefer SubtreePrefetch
        let remote = SyncHandshake::new([2; 32], 100, 5, vec![]);
        let caps = SyncCapabilities::default(); // Doesn't support SubtreePrefetch

        let selection = select_protocol_with_fallback(&local, &remote, &caps);

        // Should fall back to HashComparison since SubtreePrefetch not supported
        assert!(matches!(
            selection.protocol,
            SyncProtocol::HashComparison { .. }
        ));
        assert!(selection.reason.contains("fallback"));
    }

    // =========================================================================
    // Delta Sync Tests (Issue #1772)
    // =========================================================================

    #[test]
    fn test_delta_sync_request_roundtrip() {
        let request = DeltaSyncRequest::new(vec![[1; 32], [2; 32], [3; 32]]);

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: DeltaSyncRequest = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(request, decoded);
        assert_eq!(decoded.count(), 3);
    }

    #[test]
    fn test_delta_sync_request_threshold() {
        // Within threshold
        let small_request = DeltaSyncRequest::new(vec![[1; 32]; 10]);
        assert!(small_request.is_within_threshold());

        // At threshold
        let at_threshold = DeltaSyncRequest::new(vec![[1; 32]; DEFAULT_DELTA_SYNC_THRESHOLD]);
        assert!(at_threshold.is_within_threshold());

        // Over threshold
        let large_request = DeltaSyncRequest::new(vec![[1; 32]; DEFAULT_DELTA_SYNC_THRESHOLD + 1]);
        assert!(!large_request.is_within_threshold());
    }

    #[test]
    fn test_delta_payload_roundtrip() {
        let payload = DeltaPayload {
            id: [1; 32],
            parents: vec![[2; 32], [3; 32]],
            payload: vec![4, 5, 6, 7],
            hlc_timestamp: 12345678,
            expected_root_hash: [8; 32],
        };

        let encoded = borsh::to_vec(&payload).expect("serialize");
        let decoded: DeltaPayload = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(payload, decoded);
        assert!(!decoded.is_genesis());
    }

    #[test]
    fn test_delta_payload_genesis() {
        let genesis = DeltaPayload {
            id: [1; 32],
            parents: vec![], // No parents = genesis
            payload: vec![1, 2, 3],
            hlc_timestamp: 0,
            expected_root_hash: [2; 32],
        };

        assert!(genesis.is_genesis());

        let non_genesis = DeltaPayload {
            id: [2; 32],
            parents: vec![[1; 32]], // Has parent
            payload: vec![4, 5, 6],
            hlc_timestamp: 1,
            expected_root_hash: [3; 32],
        };

        assert!(!non_genesis.is_genesis());
    }

    #[test]
    fn test_delta_sync_response_roundtrip() {
        let delta1 = DeltaPayload {
            id: [1; 32],
            parents: vec![],
            payload: vec![1, 2, 3],
            hlc_timestamp: 100,
            expected_root_hash: [10; 32],
        };
        let delta2 = DeltaPayload {
            id: [2; 32],
            parents: vec![[1; 32]],
            payload: vec![4, 5, 6],
            hlc_timestamp: 200,
            expected_root_hash: [20; 32],
        };

        let response = DeltaSyncResponse::new(vec![delta1, delta2], vec![[99; 32]]);

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: DeltaSyncResponse = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(response, decoded);
        assert_eq!(decoded.count(), 2);
        assert!(!decoded.is_complete()); // Has not_found entries
    }

    #[test]
    fn test_delta_sync_response_complete() {
        let delta = DeltaPayload {
            id: [1; 32],
            parents: vec![],
            payload: vec![1, 2, 3],
            hlc_timestamp: 100,
            expected_root_hash: [10; 32],
        };

        let complete_response = DeltaSyncResponse::new(vec![delta], vec![]);
        assert!(complete_response.is_complete());

        let incomplete_response = DeltaSyncResponse::empty(vec![[1; 32]]);
        assert!(!incomplete_response.is_complete());
        assert_eq!(incomplete_response.count(), 0);
    }

    #[test]
    fn test_delta_apply_result() {
        let success = DeltaApplyResult::Success {
            applied_count: 5,
            new_root_hash: [1; 32],
        };
        assert!(success.is_success());
        assert!(!success.needs_state_sync());

        let missing_parents = DeltaApplyResult::MissingParents {
            missing_parent_deltas: vec![[2; 32]],
            applied_before_failure: 3,
        };
        assert!(!missing_parents.is_success());
        assert!(missing_parents.needs_state_sync());

        let failed = DeltaApplyResult::Failed {
            reason: "hash mismatch".to_string(),
        };
        assert!(!failed.is_success());
        assert!(!failed.needs_state_sync());
    }
}
