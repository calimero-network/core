#![expect(single_use_lifetimes, reason = "borsh shenanigans")]

use std::borrow::Cow;
use std::collections::HashSet;

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
pub const DEFAULT_DELTA_SYNC_THRESHOLD: usize = 128;

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
// HashComparison Sync Types (CIP §4 - State Machine, STATE-BASED branch)
// =============================================================================

/// Request to traverse the Merkle tree for hash comparison.
///
/// Used for recursive tree traversal to identify differing entities.
/// Start at root, request children, compare hashes, recurse on differences.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct TreeNodeRequest {
    /// ID of the node to request (root hash or internal node hash).
    pub node_id: [u8; 32],

    /// Maximum depth to traverse from this node (private to enforce validation).
    ///
    /// Use `depth()` accessor which always clamps to MAX_TREE_DEPTH.
    /// Use `with_depth()` constructor to set a depth limit.
    max_depth: Option<usize>,
}

impl TreeNodeRequest {
    /// Create a request for a specific node.
    #[must_use]
    pub fn new(node_id: [u8; 32]) -> Self {
        Self {
            node_id,
            max_depth: None,
        }
    }

    /// Create a request with depth limit.
    #[must_use]
    pub fn with_depth(node_id: [u8; 32], max_depth: usize) -> Self {
        Self {
            node_id,
            // Clamp to MAX_TREE_DEPTH to prevent resource exhaustion
            max_depth: Some(max_depth.min(MAX_TREE_DEPTH)),
        }
    }

    /// Create a request for the root node.
    #[must_use]
    pub fn root(root_hash: [u8; 32]) -> Self {
        Self::new(root_hash)
    }

    /// Get the validated depth limit.
    ///
    /// Always clamps to MAX_TREE_DEPTH, even if raw field was set to a larger
    /// value (e.g., via deserialization from an untrusted source).
    ///
    /// Use this instead of accessing `max_depth` directly when processing requests.
    #[must_use]
    pub fn depth(&self) -> Option<usize> {
        self.max_depth.map(|d| d.min(MAX_TREE_DEPTH))
    }
}

/// Maximum nodes per response to prevent memory exhaustion.
///
/// Limits the size of `TreeNodeResponse::nodes` to prevent DoS attacks
/// from malicious peers sending oversized responses.
pub const MAX_NODES_PER_RESPONSE: usize = 1000;

/// Response containing tree nodes for hash comparison.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct TreeNodeResponse {
    /// Nodes in the requested subtree.
    ///
    /// Limited to MAX_NODES_PER_RESPONSE entries. Use `is_valid()` to check
    /// bounds after deserialization from untrusted sources.
    pub nodes: Vec<TreeNode>,

    /// True if the requested node was not found.
    pub not_found: bool,
}

impl TreeNodeResponse {
    /// Create a response with nodes.
    #[must_use]
    pub fn new(nodes: Vec<TreeNode>) -> Self {
        Self {
            nodes,
            not_found: false,
        }
    }

    /// Create a not-found response.
    #[must_use]
    pub fn not_found() -> Self {
        Self {
            nodes: vec![],
            not_found: true,
        }
    }

    /// Check if response contains any leaf nodes.
    #[must_use]
    pub fn has_leaves(&self) -> bool {
        self.nodes.iter().any(|n| n.is_leaf())
    }

    /// Get an iterator over leaf nodes in response.
    ///
    /// Returns an iterator rather than allocating a Vec, which is more
    /// efficient for single-pass iteration.
    pub fn leaves(&self) -> impl Iterator<Item = &TreeNode> {
        self.nodes.iter().filter(|n| n.is_leaf())
    }

    /// Check if response is within valid bounds.
    ///
    /// Call this after deserializing from untrusted sources to prevent
    /// memory exhaustion attacks. Validates both response size and all
    /// contained nodes.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.nodes.len() <= MAX_NODES_PER_RESPONSE && self.nodes.iter().all(TreeNode::is_valid)
    }
}

/// Maximum children per node (typical Merkle trees use binary or small fanout).
///
/// This limit prevents memory exhaustion from malicious nodes with excessive children.
pub const MAX_CHILDREN_PER_NODE: usize = 256;

/// A node in the Merkle tree.
///
/// Can be either an internal node (has children) or a leaf node (has data).
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct TreeNode {
    /// Node ID - stable identifier derived from the node's position/key in the tree.
    ///
    /// For internal nodes: typically hash of path or concatenation of child keys.
    /// For leaf nodes: typically hash of the entity key.
    /// This ID remains stable even when content changes.
    pub id: [u8; 32],

    /// Merkle hash - changes when subtree content changes.
    ///
    /// For internal nodes: hash of all children's hashes (propagates changes up).
    /// For leaf nodes: hash of the leaf data (key + value + metadata).
    /// Used for efficient comparison: if hashes match, subtrees are identical.
    pub hash: [u8; 32],

    /// Child node IDs (empty for leaf nodes).
    ///
    /// Typically limited to MAX_CHILDREN_PER_NODE. Use `is_valid()` to check
    /// bounds after deserialization from untrusted sources.
    pub children: Vec<[u8; 32]>,

    /// Leaf data (present only for leaf nodes).
    pub leaf_data: Option<TreeLeafData>,
}

impl TreeNode {
    /// Create an internal node.
    #[must_use]
    pub fn internal(id: [u8; 32], hash: [u8; 32], children: Vec<[u8; 32]>) -> Self {
        Self {
            id,
            hash,
            children,
            leaf_data: None,
        }
    }

    /// Create a leaf node.
    #[must_use]
    pub fn leaf(id: [u8; 32], hash: [u8; 32], data: TreeLeafData) -> Self {
        Self {
            id,
            hash,
            children: vec![],
            leaf_data: Some(data),
        }
    }

    /// Check if node is within valid bounds and structurally valid.
    ///
    /// Call this after deserializing from untrusted sources.
    /// Validates:
    /// - Children count within MAX_CHILDREN_PER_NODE
    /// - Structural invariant: must have exactly one of children OR leaf_data
    /// - Leaf data validity (value size within limits)
    #[must_use]
    pub fn is_valid(&self) -> bool {
        // Check children count
        if self.children.len() > MAX_CHILDREN_PER_NODE {
            return false;
        }

        // Check structural invariant: must have exactly one of children or data
        // - Internal nodes: non-empty children, no leaf_data
        // - Leaf nodes: empty children, has leaf_data
        let has_children = !self.children.is_empty();
        let has_data = self.leaf_data.is_some();
        if has_children == has_data {
            // Invalid: either both present (ambiguous) or both absent (empty)
            return false;
        }

        // Validate leaf data if present
        if let Some(ref leaf_data) = self.leaf_data {
            if !leaf_data.is_valid() {
                return false;
            }
        }

        true
    }

    /// Check if this is a leaf node.
    #[must_use]
    pub fn is_leaf(&self) -> bool {
        self.leaf_data.is_some()
    }

    /// Check if this is an internal node.
    #[must_use]
    pub fn is_internal(&self) -> bool {
        self.leaf_data.is_none()
    }

    /// Get number of children (0 for leaf nodes).
    #[must_use]
    pub fn child_count(&self) -> usize {
        self.children.len()
    }
}

/// Maximum size for leaf value data (1 MB).
///
/// Prevents memory exhaustion from malicious peers sending oversized leaf values.
/// This should be sufficient for most entity data while protecting against DoS.
pub const MAX_LEAF_VALUE_SIZE: usize = 1_048_576;

/// Data stored at a leaf node (entity).
///
/// Contains ALL information needed for CRDT merge on the receiving side.
/// CRITICAL: `metadata` MUST include `crdt_type` for proper merge.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct TreeLeafData {
    /// Entity key (unique identifier within collection).
    pub key: [u8; 32],

    /// Serialized entity value.
    pub value: Vec<u8>,

    /// Entity metadata including crdt_type.
    /// CRITICAL: Must be included for CRDT merge to work correctly.
    pub metadata: LeafMetadata,
}

impl TreeLeafData {
    /// Create leaf data.
    #[must_use]
    pub fn new(key: [u8; 32], value: Vec<u8>, metadata: LeafMetadata) -> Self {
        Self {
            key,
            value,
            metadata,
        }
    }

    /// Check if leaf data is within valid bounds.
    ///
    /// Call this after deserializing from untrusted sources.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.value.len() <= MAX_LEAF_VALUE_SIZE
    }
}

/// Metadata for a leaf entity.
///
/// Minimal metadata needed for CRDT merge during sync.
///
/// This is a wire-protocol-optimized subset of `calimero_storage::Metadata`.
/// It contains only the fields needed for sync operations, avoiding larger
/// fields like `field_name: String` that aren't needed over the wire.
///
/// When receiving entities, implementations should map this to/from the
/// storage layer's `Metadata` type.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct LeafMetadata {
    /// CRDT type for proper merge semantics.
    pub crdt_type: CrdtType,

    /// HLC timestamp of last modification.
    pub hlc_timestamp: u64,

    /// Version counter (for some CRDT types).
    pub version: u64,

    /// Collection ID this entity belongs to.
    pub collection_id: [u8; 32],

    /// Optional parent entity ID (for nested structures).
    pub parent_id: Option<[u8; 32]>,
}

impl LeafMetadata {
    /// Create metadata with required fields.
    #[must_use]
    pub fn new(crdt_type: CrdtType, hlc_timestamp: u64, collection_id: [u8; 32]) -> Self {
        Self {
            crdt_type,
            hlc_timestamp,
            version: 0,
            collection_id,
            parent_id: None,
        }
    }

    /// Set version.
    #[must_use]
    pub fn with_version(mut self, version: u64) -> Self {
        self.version = version;
        self
    }

    /// Set parent ID.
    #[must_use]
    pub fn with_parent(mut self, parent_id: [u8; 32]) -> Self {
        self.parent_id = Some(parent_id);
        self
    }
}

// Re-export the unified CrdtType from primitives (consolidated per issue #1912)
pub use calimero_primitives::crdt::CrdtType;

/// Result of comparing two tree nodes.
///
/// Used for Merkle tree traversal during HashComparison sync.
/// Identifies which children need further traversal in both directions.
///
/// Note: Borsh derives are included for consistency with other sync types and
/// potential future use in batched comparison responses over the wire.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub enum TreeCompareResult {
    /// Hashes match - no sync needed for this subtree.
    Equal,
    /// Hashes differ - need to recurse or fetch leaf.
    ///
    /// For internal nodes: lists children to recurse into.
    /// For leaf nodes: all vecs will be empty, but Different still indicates
    /// that the leaf data needs to be fetched and merged bidirectionally.
    Different {
        /// IDs of children in remote but not in local (need to fetch).
        remote_only_children: Vec<[u8; 32]>,
        /// IDs of children in local but not in remote (for bidirectional sync).
        local_only_children: Vec<[u8; 32]>,
        /// IDs of children present on both sides that need recursive comparison.
        /// These are the primary candidates for recursion when parent hashes differ.
        common_children: Vec<[u8; 32]>,
    },
    /// Local node missing - need to fetch from remote.
    LocalMissing,
    /// Remote node missing - local has data that remote doesn't.
    /// For bidirectional sync, this means we may need to push to remote.
    RemoteMissing,
}

impl TreeCompareResult {
    /// Check if sync (pull from remote) is needed.
    ///
    /// Returns true if local needs data from remote.
    #[must_use]
    pub fn needs_sync(&self) -> bool {
        !matches!(self, Self::Equal | Self::RemoteMissing)
    }

    /// Check if push (send to remote) is needed for bidirectional sync.
    ///
    /// Returns true if local has data that remote doesn't:
    /// - `RemoteMissing`: entire local subtree needs pushing
    /// - `Different` with `local_only_children`: those children need pushing
    /// - `Different` with all empty vecs: this is a **leaf node comparison** where
    ///   hashes differ, meaning local leaf data needs pushing for CRDT merge
    #[must_use]
    pub fn needs_push(&self) -> bool {
        match self {
            Self::RemoteMissing => true,
            Self::Different {
                remote_only_children,
                local_only_children,
                common_children,
            } => {
                // Push needed if we have local-only children
                if !local_only_children.is_empty() {
                    return true;
                }
                // Leaf node detection: when all child vecs are empty but hashes differed,
                // we compared two leaf nodes with different content. The local leaf data
                // needs to be pushed for bidirectional CRDT merge.
                remote_only_children.is_empty() && common_children.is_empty()
            }
            _ => false,
        }
    }
}

/// Maximum allowed tree depth for traversal requests.
///
/// This limit prevents resource exhaustion from malicious peers requesting
/// extremely deep traversals. Most practical Merkle trees have depth < 32.
pub const MAX_TREE_DEPTH: usize = 64;

/// Compare local and remote tree nodes.
///
/// Returns which children (if any) need further traversal in both directions.
/// This supports bidirectional sync where both nodes may have unique data.
///
/// # Arguments
/// * `local` - Local tree node, or None if not present locally
/// * `remote` - Remote tree node, or None if not present on remote
///
/// # Precondition
/// When both nodes are present, they must represent the same tree position
/// (i.e., have matching IDs). Comparing nodes at different positions is a
/// caller bug and will trigger a debug assertion.
///
/// # Returns
/// * `Equal` - Hashes match, no sync needed
/// * `Different` - Hashes differ, contains children needing traversal
/// * `LocalMissing` - Need to fetch from remote
/// * `RemoteMissing` - Local has data remote doesn't (for bidirectional push)
#[must_use]
pub fn compare_tree_nodes(
    local: Option<&TreeNode>,
    remote: Option<&TreeNode>,
) -> TreeCompareResult {
    match (local, remote) {
        (None, None) => TreeCompareResult::Equal,
        (None, Some(_)) => TreeCompareResult::LocalMissing,
        (Some(_), None) => TreeCompareResult::RemoteMissing,
        (Some(local_node), Some(remote_node)) => {
            // Verify precondition: nodes must represent the same tree position
            debug_assert_eq!(
                local_node.id, remote_node.id,
                "compare_tree_nodes called with nodes at different tree positions"
            );

            if local_node.hash == remote_node.hash {
                TreeCompareResult::Equal
            } else {
                // Use HashSet for O(1) lookups instead of O(n) Vec::contains
                let local_children: HashSet<&[u8; 32]> = local_node.children.iter().collect();
                let remote_children: HashSet<&[u8; 32]> = remote_node.children.iter().collect();

                // Children in remote but not in local (need to fetch)
                let remote_only_children: Vec<[u8; 32]> = remote_node
                    .children
                    .iter()
                    .filter(|child_id| !local_children.contains(child_id))
                    .copied()
                    .collect();

                // Children in local but not in remote (for bidirectional sync)
                let local_only_children: Vec<[u8; 32]> = local_node
                    .children
                    .iter()
                    .filter(|child_id| !remote_children.contains(child_id))
                    .copied()
                    .collect();

                // Children present on both sides - these are the primary candidates
                // for recursive comparison when parent hashes differ
                let common_children: Vec<[u8; 32]> = local_node
                    .children
                    .iter()
                    .filter(|child_id| remote_children.contains(child_id))
                    .copied()
                    .collect();

                TreeCompareResult::Different {
                    remote_only_children,
                    local_only_children,
                    common_children,
                }
            }
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

    // =========================================================================
    // HashComparison Sync Tests (Issue #1774)
    // =========================================================================

    #[test]
    fn test_tree_node_request_roundtrip() {
        let request = TreeNodeRequest::with_depth([1; 32], 3);

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: TreeNodeRequest = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(request, decoded);
        assert_eq!(decoded.max_depth, Some(3));
    }

    #[test]
    fn test_tree_node_request_root() {
        let root_hash = [42; 32];
        let request = TreeNodeRequest::root(root_hash);

        assert_eq!(request.node_id, root_hash);
        assert!(request.max_depth.is_none());
    }

    #[test]
    fn test_tree_node_internal() {
        let node = TreeNode::internal([1; 32], [2; 32], vec![[3; 32], [4; 32]]);

        assert!(node.is_internal());
        assert!(!node.is_leaf());
        assert_eq!(node.child_count(), 2);
        assert!(node.leaf_data.is_none());
    }

    #[test]
    fn test_tree_node_leaf() {
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 12345, [5; 32]);
        let leaf_data = TreeLeafData::new([1; 32], vec![1, 2, 3], metadata);
        let node = TreeNode::leaf([2; 32], [3; 32], leaf_data);

        assert!(node.is_leaf());
        assert!(!node.is_internal());
        assert_eq!(node.child_count(), 0);
        assert!(node.leaf_data.is_some());
    }

    #[test]
    fn test_tree_node_roundtrip() {
        let metadata = LeafMetadata::new(CrdtType::UnorderedMap, 999, [6; 32])
            .with_version(5)
            .with_parent([7; 32]);
        let leaf_data = TreeLeafData::new([1; 32], vec![4, 5, 6], metadata);
        let node = TreeNode::leaf([2; 32], [3; 32], leaf_data);

        let encoded = borsh::to_vec(&node).expect("serialize");
        let decoded: TreeNode = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(node, decoded);
    }

    #[test]
    fn test_tree_node_response_roundtrip() {
        let internal = TreeNode::internal([1; 32], [2; 32], vec![[3; 32]]);
        let metadata = LeafMetadata::new(CrdtType::Rga, 100, [4; 32]);
        let leaf_data = TreeLeafData::new([5; 32], vec![7, 8, 9], metadata);
        let leaf = TreeNode::leaf([6; 32], [7; 32], leaf_data);

        let response = TreeNodeResponse::new(vec![internal, leaf]);

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: TreeNodeResponse = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(response, decoded);
        assert!(decoded.has_leaves());
        assert_eq!(decoded.leaves().count(), 1);
    }

    #[test]
    fn test_tree_node_response_not_found() {
        let response = TreeNodeResponse::not_found();

        assert!(response.not_found);
        assert!(response.nodes.is_empty());
        assert!(!response.has_leaves());
    }

    #[test]
    fn test_leaf_metadata_builder() {
        let metadata = LeafMetadata::new(CrdtType::Counter, 500, [1; 32])
            .with_version(10)
            .with_parent([2; 32]);

        assert_eq!(metadata.crdt_type, CrdtType::Counter);
        assert_eq!(metadata.hlc_timestamp, 500);
        assert_eq!(metadata.version, 10);
        assert_eq!(metadata.parent_id, Some([2; 32]));
    }

    #[test]
    fn test_crdt_type_variants() {
        // Test all variants in declaration order (discriminant order matters for Borsh)
        let types = vec![
            CrdtType::LwwRegister,
            CrdtType::Counter,
            CrdtType::Rga,
            CrdtType::UnorderedMap,
            CrdtType::UnorderedSet,
            CrdtType::Vector,
            CrdtType::UserStorage,
            CrdtType::FrozenStorage,
            CrdtType::Record,
            CrdtType::Custom("test".to_string()),
            CrdtType::LwwSet,
            CrdtType::OrSet,
        ];

        for crdt_type in types {
            let encoded = borsh::to_vec(&crdt_type).expect("serialize");
            let decoded: CrdtType = borsh::from_slice(&encoded).expect("deserialize");
            assert_eq!(crdt_type, decoded);
        }
    }

    #[test]
    fn test_compare_tree_nodes_equal() {
        let local = TreeNode::internal([1; 32], [99; 32], vec![[2; 32]]);
        let remote = TreeNode::internal([1; 32], [99; 32], vec![[2; 32]]);

        let result = compare_tree_nodes(Some(&local), Some(&remote));
        assert_eq!(result, TreeCompareResult::Equal);
        assert!(!result.needs_sync());
    }

    #[test]
    fn test_compare_tree_nodes_local_missing() {
        let remote = TreeNode::internal([1; 32], [2; 32], vec![[3; 32]]);

        let result = compare_tree_nodes(None, Some(&remote));
        assert_eq!(result, TreeCompareResult::LocalMissing);
        assert!(result.needs_sync());
    }

    #[test]
    fn test_compare_tree_nodes_different() {
        let local = TreeNode::internal([1; 32], [10; 32], vec![[2; 32]]);
        let remote = TreeNode::internal([1; 32], [20; 32], vec![[2; 32], [3; 32]]);

        let result = compare_tree_nodes(Some(&local), Some(&remote));

        match &result {
            TreeCompareResult::Different {
                remote_only_children,
                local_only_children: _,
                common_children,
            } => {
                // [3; 32] is in remote but not in local
                assert!(remote_only_children.contains(&[3; 32]));
                // [2; 32] is common to both sides
                assert!(common_children.contains(&[2; 32]));
            }
            _ => panic!("Expected Different result"),
        }
        assert!(result.needs_sync());
    }

    #[test]
    fn test_tree_compare_result_needs_sync() {
        assert!(!TreeCompareResult::Equal.needs_sync());
        assert!(!TreeCompareResult::RemoteMissing.needs_sync());
        assert!(TreeCompareResult::LocalMissing.needs_sync());
        assert!(TreeCompareResult::Different {
            remote_only_children: vec![],
            local_only_children: vec![],
            common_children: vec![],
        }
        .needs_sync());
    }

    #[test]
    fn test_tree_compare_result_roundtrip() {
        // Test all variants survive Borsh serialization roundtrip
        let variants = vec![
            TreeCompareResult::Equal,
            TreeCompareResult::LocalMissing,
            TreeCompareResult::RemoteMissing,
            TreeCompareResult::Different {
                remote_only_children: vec![[1; 32], [2; 32]],
                local_only_children: vec![[3; 32]],
                common_children: vec![[4; 32], [5; 32], [6; 32]],
            },
            TreeCompareResult::Different {
                remote_only_children: vec![],
                local_only_children: vec![],
                common_children: vec![],
            },
        ];

        for original in variants {
            let encoded = borsh::to_vec(&original).expect("encode");
            let decoded: TreeCompareResult = borsh::from_slice(&encoded).expect("decode");
            assert_eq!(original, decoded);
        }
    }

    // =========================================================================
    // Bug Fix Tests (AI Review Findings)
    // =========================================================================

    #[test]
    fn test_compare_tree_nodes_leaf_content_differs() {
        // CRITICAL: When both are leaves with different hashes but no children,
        // we must still identify that sync is needed.
        let local_metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let local_leaf = TreeLeafData::new([10; 32], vec![1, 2, 3], local_metadata);
        let local = TreeNode::leaf([1; 32], [100; 32], local_leaf); // hash = [100; 32]

        let remote_metadata = LeafMetadata::new(CrdtType::LwwRegister, 200, [1; 32]);
        let remote_leaf = TreeLeafData::new([10; 32], vec![4, 5, 6], remote_metadata);
        let remote = TreeNode::leaf([1; 32], [200; 32], remote_leaf); // hash = [200; 32]

        let result = compare_tree_nodes(Some(&local), Some(&remote));

        // Both are leaves, hashes differ, but no children
        // The function MUST indicate that this node needs sync AND push
        match &result {
            TreeCompareResult::Different {
                remote_only_children,
                local_only_children,
                common_children,
            } => {
                // For leaf nodes, all child vecs will be empty (no children)
                // but the Different result itself indicates sync is needed
                assert!(remote_only_children.is_empty());
                assert!(local_only_children.is_empty());
                assert!(common_children.is_empty());
            }
            _ => panic!("Expected Different result for leaves with different content"),
        }
        assert!(result.needs_sync());
        // Bug fix: needs_push should now return true for differing leaf nodes
        assert!(result.needs_push());
    }

    #[test]
    fn test_compare_tree_nodes_remote_missing() {
        // When local has a node but remote doesn't - bidirectional sync
        let local = TreeNode::internal([1; 32], [2; 32], vec![[3; 32]]);

        let result = compare_tree_nodes(Some(&local), None);
        assert_eq!(result, TreeCompareResult::RemoteMissing);
        assert!(!result.needs_sync()); // Nothing to pull from remote
    }

    #[test]
    fn test_compare_tree_nodes_local_only_children() {
        // Local has children that remote doesn't have - for bidirectional sync
        let local = TreeNode::internal([1; 32], [10; 32], vec![[2; 32], [3; 32], [4; 32]]);
        let remote = TreeNode::internal([1; 32], [20; 32], vec![[2; 32], [5; 32]]);

        let result = compare_tree_nodes(Some(&local), Some(&remote));

        match &result {
            TreeCompareResult::Different {
                remote_only_children,
                local_only_children,
                common_children,
            } => {
                // [5; 32] is in remote but not in local
                assert!(remote_only_children.contains(&[5; 32]));
                // [3; 32] and [4; 32] are in local but not in remote
                assert!(local_only_children.contains(&[3; 32]));
                assert!(local_only_children.contains(&[4; 32]));
                // [2; 32] is common to both sides
                assert!(common_children.contains(&[2; 32]));
            }
            _ => panic!("Expected Different result"),
        }
    }

    #[test]
    fn test_tree_node_request_max_depth_validation() {
        // MAX_TREE_DEPTH should be enforced
        let request = TreeNodeRequest::with_depth([1; 32], MAX_TREE_DEPTH);
        assert_eq!(request.max_depth, Some(MAX_TREE_DEPTH));

        // Excessive depth should be clamped
        let excessive = TreeNodeRequest::with_depth([1; 32], MAX_TREE_DEPTH + 100);
        assert_eq!(excessive.max_depth, Some(MAX_TREE_DEPTH));
    }

    #[test]
    fn test_tree_node_request_depth_accessor() {
        // depth() should always clamp, even if raw field is excessive
        let mut request = TreeNodeRequest::new([1; 32]);
        request.max_depth = Some(usize::MAX); // Simulate malicious deserialization

        // depth() should clamp to MAX_TREE_DEPTH
        assert_eq!(request.depth(), Some(MAX_TREE_DEPTH));

        // None should remain None
        let request_none = TreeNodeRequest::new([1; 32]);
        assert_eq!(request_none.depth(), None);
    }

    #[test]
    fn test_tree_node_response_validation() {
        // Valid response with internal node
        let valid_response =
            TreeNodeResponse::new(vec![TreeNode::internal([1; 32], [2; 32], vec![[3; 32]])]);
        assert!(valid_response.is_valid());

        // Valid response with leaf node
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let leaf_data = TreeLeafData::new([10; 32], vec![1, 2, 3], metadata);
        let leaf_response =
            TreeNodeResponse::new(vec![TreeNode::leaf([1; 32], [2; 32], leaf_data)]);
        assert!(leaf_response.is_valid());

        // Response at limit is valid
        let mut nodes = Vec::new();
        for i in 0..MAX_NODES_PER_RESPONSE {
            let id = [i as u8; 32];
            // Use internal nodes with at least one child
            nodes.push(TreeNode::internal(id, id, vec![[0; 32]]));
        }
        let at_limit = TreeNodeResponse::new(nodes);
        assert!(at_limit.is_valid());
    }

    #[test]
    fn test_tree_node_validation() {
        // Valid node with few children
        let valid = TreeNode::internal([1; 32], [2; 32], vec![[3; 32], [4; 32]]);
        assert!(valid.is_valid());

        // Node at limit is valid
        let children: Vec<[u8; 32]> = (0..MAX_CHILDREN_PER_NODE).map(|i| [i as u8; 32]).collect();
        let at_limit = TreeNode::internal([1; 32], [2; 32], children);
        assert!(at_limit.is_valid());

        // Node exceeding limit is invalid
        let over_children: Vec<[u8; 32]> =
            (0..=MAX_CHILDREN_PER_NODE).map(|i| [i as u8; 32]).collect();
        let over_limit = TreeNode::internal([1; 32], [2; 32], over_children);
        assert!(!over_limit.is_valid());

        // Node with both children AND leaf_data is invalid (structural invariant)
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let leaf_data = TreeLeafData::new([10; 32], vec![1, 2, 3], metadata);
        let invalid_node = TreeNode {
            id: [1; 32],
            hash: [2; 32],
            children: vec![[3; 32]],
            leaf_data: Some(leaf_data),
        };
        assert!(!invalid_node.is_valid());

        // Valid leaf node
        let valid_metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);
        let valid_leaf_data = TreeLeafData::new([10; 32], vec![1, 2, 3], valid_metadata);
        let valid_leaf = TreeNode::leaf([1; 32], [2; 32], valid_leaf_data);
        assert!(valid_leaf.is_valid());

        // Empty internal node (no children, no leaf_data) is invalid
        let empty_node = TreeNode::internal([1; 32], [2; 32], vec![]);
        assert!(!empty_node.is_valid());
    }

    #[test]
    fn test_tree_node_response_validation_over_limit() {
        // Response exceeding limit is invalid
        let mut nodes = Vec::new();
        for i in 0..=MAX_NODES_PER_RESPONSE {
            let id = [i as u8; 32];
            // Use internal nodes with at least one child
            nodes.push(TreeNode::internal(id, id, vec![[0; 32]]));
        }
        let over_limit = TreeNodeResponse::new(nodes);
        assert!(!over_limit.is_valid());

        // Response with invalid nested node is invalid (too many children)
        let over_children: Vec<[u8; 32]> =
            (0..=MAX_CHILDREN_PER_NODE).map(|i| [i as u8; 32]).collect();
        let invalid_node = TreeNode::internal([1; 32], [2; 32], over_children);
        let response_with_invalid = TreeNodeResponse::new(vec![invalid_node]);
        assert!(!response_with_invalid.is_valid());

        // Response with empty internal node is invalid
        let empty_node = TreeNode::internal([1; 32], [2; 32], vec![]);
        let response_with_empty = TreeNodeResponse::new(vec![empty_node]);
        assert!(!response_with_empty.is_valid());
    }

    #[test]
    fn test_tree_leaf_data_validation() {
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32]);

        // Valid leaf data
        let valid = TreeLeafData::new([1; 32], vec![1, 2, 3], metadata.clone());
        assert!(valid.is_valid());

        // Leaf data at limit is valid
        let at_limit_value = vec![0u8; MAX_LEAF_VALUE_SIZE];
        let at_limit = TreeLeafData::new([1; 32], at_limit_value, metadata.clone());
        assert!(at_limit.is_valid());

        // Leaf data exceeding limit is invalid
        let over_limit_value = vec![0u8; MAX_LEAF_VALUE_SIZE + 1];
        let over_limit = TreeLeafData::new([1; 32], over_limit_value, metadata);
        assert!(!over_limit.is_valid());
    }

    #[test]
    fn test_tree_compare_result_needs_push() {
        // RemoteMissing needs push
        assert!(TreeCompareResult::RemoteMissing.needs_push());

        // Different with local_only_children needs push
        let with_local_only = TreeCompareResult::Different {
            remote_only_children: vec![],
            local_only_children: vec![[1; 32]],
            common_children: vec![],
        };
        assert!(with_local_only.needs_push());

        // Different with only remote_only_children (internal node) doesn't need push
        let with_remote_only = TreeCompareResult::Different {
            remote_only_children: vec![[1; 32]],
            local_only_children: vec![],
            common_children: vec![],
        };
        assert!(!with_remote_only.needs_push());

        // Different with only common_children (internal node) doesn't need push
        // - caller needs to recurse into common_children to determine if push is needed
        let with_common_only = TreeCompareResult::Different {
            remote_only_children: vec![],
            local_only_children: vec![],
            common_children: vec![[1; 32]],
        };
        assert!(!with_common_only.needs_push());

        // Different for leaf nodes (all vecs empty) DOES need push
        // This is a critical fix: differing leaves have unique data to push
        let differing_leaves = TreeCompareResult::Different {
            remote_only_children: vec![],
            local_only_children: vec![],
            common_children: vec![],
        };
        assert!(differing_leaves.needs_push());

        // Equal doesn't need push
        assert!(!TreeCompareResult::Equal.needs_push());

        // LocalMissing doesn't need push
        assert!(!TreeCompareResult::LocalMissing.needs_push());
    }
}
