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
    let diff = (local.entity_count as i64 - remote.entity_count as i64).unsigned_abs();
    let denominator = remote.entity_count.max(1);
    diff as f64 / denominator as f64
}

/// Select the optimal sync protocol based on handshake information.
///
/// Implements the decision table from CIP §2.3:
///
/// | # | Condition | Selected Protocol |
/// |---|-----------|-------------------|
/// | 1 | `local.root_hash == remote.root_hash` | `None` |
/// | 2 | `!local.has_state` (fresh node) | `Snapshot` |
/// | 3 | `local.has_state` AND divergence > 50% | `HashComparison` |
/// | 4 | `max_depth > 3` AND divergence < 20% | `SubtreePrefetch` |
/// | 5 | `entity_count > 50` AND divergence < 10% | `BloomFilter` |
/// | 6 | `max_depth <= 2` AND many children | `LevelWise` |
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
                filter_size: (remote.entity_count * 10).min(10_000), // ~10 bits per entity, max 10k
                false_positive_rate: 0.01,
            },
            reason: "large tree with small divergence, using bloom filter",
        };
    }

    // Rule 6: Wide shallow tree
    // "Many children" heuristic: entity_count / max_depth > 10
    let avg_children_per_level = if remote.max_depth > 0 {
        remote.entity_count / remote.max_depth
    } else {
        remote.entity_count
    };

    if remote.max_depth <= 2 && avg_children_per_level > 10 {
        return ProtocolSelection {
            protocol: SyncProtocol::LevelWise {
                max_depth: remote.max_depth,
            },
            reason: "wide shallow tree, using level-wise sync",
        };
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

/// Check if a protocol is supported by the remote peer.
#[must_use]
pub fn is_protocol_supported(protocol: &SyncProtocol, capabilities: &SyncCapabilities) -> bool {
    capabilities.supported_protocols.iter().any(|p| {
        // Match on variant, ignoring inner values
        std::mem::discriminant(p) == std::mem::discriminant(protocol)
    })
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
pub const DEFAULT_DELTA_SYNC_THRESHOLD: usize = 50;

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
    /// Used for verification (may differ if concurrent deltas exist).
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
// Delta Buffering During Sync (CIP §5 - Delta Handling During Sync)
// =============================================================================

/// Default buffer capacity for deltas during state sync.
///
/// This should be large enough to handle deltas arriving during a typical
/// state transfer. If exceeded, deltas are NOT dropped (invariant I6), but
/// a warning is logged.
pub const DEFAULT_BUFFER_CAPACITY: usize = 1000;

/// State of an ongoing synchronization.
#[derive(Clone, Debug, PartialEq)]
pub enum SyncState {
    /// No sync in progress.
    Idle,
    /// Handshake sent, waiting for response.
    Handshaking,
    /// Receiving state data (HashComparison, Snapshot, etc.).
    ReceivingState,
    /// State received, replaying buffered deltas.
    ReplayingDeltas,
    /// Sync completed successfully.
    Completed,
    /// Sync failed with error.
    Failed(String),
}

impl SyncState {
    /// Check if sync is actively in progress.
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Handshaking | Self::ReceivingState | Self::ReplayingDeltas
        )
    }

    /// Check if deltas should be buffered (sync receiving state).
    #[must_use]
    pub fn should_buffer_deltas(&self) -> bool {
        matches!(self, Self::ReceivingState)
    }
}

/// A delta buffered during state synchronization.
///
/// Contains ALL fields required for replay via DAG insertion.
/// See CIP §5 and Bug 7 in POC-IMPLEMENTATION-NOTES.md.
///
/// CRITICAL: Deltas MUST be replayed via DAG insertion (causal order),
/// NOT by HLC timestamp sorting.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct BufferedDelta {
    /// Unique delta ID (content hash).
    pub id: [u8; 32],

    /// Parent delta IDs (for causal ordering via DAG).
    pub parents: Vec<[u8; 32]>,

    /// HLC timestamp when the delta was created.
    pub hlc: u64,

    /// Nonce for decryption (24 bytes for XChaCha20-Poly1305).
    pub nonce: [u8; 24],

    /// Author's public key (for signature verification).
    pub author_id: [u8; 32],

    /// Expected root hash after applying this delta.
    pub root_hash: [u8; 32],

    /// Serialized delta payload (operations).
    pub payload: Vec<u8>,

    /// Serialized events emitted by this delta.
    pub events: Vec<Vec<u8>>,
}

impl BufferedDelta {
    /// Create a new buffered delta.
    #[must_use]
    pub fn new(
        id: [u8; 32],
        parents: Vec<[u8; 32]>,
        hlc: u64,
        nonce: [u8; 24],
        author_id: [u8; 32],
        root_hash: [u8; 32],
        payload: Vec<u8>,
        events: Vec<Vec<u8>>,
    ) -> Self {
        Self {
            id,
            parents,
            hlc,
            nonce,
            author_id,
            root_hash,
            payload,
            events,
        }
    }

    /// Check if this is a genesis delta (no parents).
    #[must_use]
    pub fn is_genesis(&self) -> bool {
        self.parents.is_empty()
    }
}

/// Context for an ongoing synchronization session.
///
/// Manages buffering of incoming deltas during state transfer.
/// Deltas are NEVER dropped (invariant I6 - liveness guarantee).
#[derive(Clone, Debug)]
pub struct SyncContext {
    /// Current sync state.
    pub state: SyncState,

    /// Deltas buffered during state transfer.
    /// Replay via DAG insertion after state is applied.
    pub buffered_deltas: Vec<BufferedDelta>,

    /// Maximum buffer capacity (soft limit - logs warning if exceeded).
    pub buffer_capacity: usize,

    /// Timestamp when sync started (for metrics).
    pub sync_start_timestamp: u64,

    /// Peer we're syncing with.
    pub peer_id: Option<[u8; 32]>,

    /// Protocol being used for this sync.
    pub protocol: SyncProtocol,
}

impl Default for SyncContext {
    fn default() -> Self {
        Self {
            state: SyncState::Idle,
            buffered_deltas: Vec::new(),
            buffer_capacity: DEFAULT_BUFFER_CAPACITY,
            sync_start_timestamp: 0,
            peer_id: None,
            protocol: SyncProtocol::None,
        }
    }
}

impl SyncContext {
    /// Create a new sync context with default capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new sync context with custom capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer_capacity: capacity,
            ..Self::default()
        }
    }

    /// Start a sync session with a peer.
    pub fn start(&mut self, peer_id: [u8; 32], protocol: SyncProtocol, timestamp: u64) {
        self.state = SyncState::Handshaking;
        self.peer_id = Some(peer_id);
        self.protocol = protocol;
        self.sync_start_timestamp = timestamp;
        self.buffered_deltas.clear();
    }

    /// Transition to receiving state.
    pub fn begin_receiving(&mut self) {
        self.state = SyncState::ReceivingState;
    }

    /// Buffer a delta during state transfer.
    ///
    /// Returns `true` if buffer is within capacity, `false` if exceeded
    /// (delta is still buffered - caller should log warning).
    ///
    /// INVARIANT I6: Deltas are NEVER dropped.
    pub fn buffer_delta(&mut self, delta: BufferedDelta) -> bool {
        let within_capacity = self.buffered_deltas.len() < self.buffer_capacity;
        self.buffered_deltas.push(delta);
        within_capacity
    }

    /// Begin replay phase (after state is applied).
    pub fn begin_replay(&mut self) {
        self.state = SyncState::ReplayingDeltas;
    }

    /// Take buffered deltas for replay.
    ///
    /// IMPORTANT: These MUST be replayed via DAG insertion (causal order),
    /// NOT by HLC timestamp sorting.
    pub fn take_buffered_deltas(&mut self) -> Vec<BufferedDelta> {
        std::mem::take(&mut self.buffered_deltas)
    }

    /// Complete the sync successfully.
    pub fn complete(&mut self) {
        self.state = SyncState::Completed;
    }

    /// Mark sync as failed.
    pub fn fail(&mut self, reason: String) {
        self.state = SyncState::Failed(reason);
    }

    /// Reset context for a new sync session.
    pub fn reset(&mut self) {
        self.state = SyncState::Idle;
        self.buffered_deltas.clear();
        self.peer_id = None;
        self.protocol = SyncProtocol::None;
        self.sync_start_timestamp = 0;
    }

    /// Check if sync is in progress.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state.is_active()
    }

    /// Check if deltas should be buffered.
    #[must_use]
    pub fn should_buffer(&self) -> bool {
        self.state.should_buffer_deltas()
    }

    /// Get number of buffered deltas.
    #[must_use]
    pub fn buffered_count(&self) -> usize {
        self.buffered_deltas.len()
    }

    /// Check if buffer has exceeded capacity (soft limit).
    #[must_use]
    pub fn is_buffer_exceeded(&self) -> bool {
        self.buffered_deltas.len() > self.buffer_capacity
    }
}

/// Metrics for delta buffering during sync.
#[derive(Clone, Debug, Default)]
pub struct BufferMetrics {
    /// Total deltas buffered in this session.
    pub total_buffered: usize,
    /// Peak buffer size reached.
    pub peak_buffer_size: usize,
    /// Number of times buffer exceeded capacity (soft warnings).
    pub capacity_exceeded_count: usize,
    /// Total deltas replayed.
    pub total_replayed: usize,
    /// Duration of sync in milliseconds (set on completion).
    pub sync_duration_ms: Option<u64>,
}

impl BufferMetrics {
    /// Record a delta being buffered.
    pub fn record_buffer(&mut self, current_size: usize) {
        self.total_buffered += 1;
        self.peak_buffer_size = self.peak_buffer_size.max(current_size);
    }

    /// Record buffer capacity exceeded.
    pub fn record_exceeded(&mut self) {
        self.capacity_exceeded_count += 1;
    }

    /// Record deltas being replayed.
    pub fn record_replay(&mut self, count: usize) {
        self.total_replayed += count;
    }

    /// Record sync completion with duration.
    pub fn record_completion(&mut self, duration_ms: u64) {
        self.sync_duration_ms = Some(duration_ms);
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

    /// Maximum depth to traverse (None = unlimited).
    /// Useful for batching: request multiple levels at once.
    pub max_depth: Option<usize>,
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
            max_depth: Some(max_depth),
        }
    }

    /// Create a request for the root node.
    #[must_use]
    pub fn root(root_hash: [u8; 32]) -> Self {
        Self::new(root_hash)
    }
}

/// Response containing tree nodes for hash comparison.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct TreeNodeResponse {
    /// Nodes in the requested subtree.
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

    /// Get all leaf nodes from response.
    #[must_use]
    pub fn leaves(&self) -> Vec<&TreeNode> {
        self.nodes.iter().filter(|n| n.is_leaf()).collect()
    }
}

/// A node in the Merkle tree.
///
/// Can be either an internal node (has children) or a leaf node (has data).
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct TreeNode {
    /// Node ID (hash of this node's content).
    pub id: [u8; 32],

    /// Merkle hash (hash of children hashes or leaf content).
    pub hash: [u8; 32],

    /// Child node IDs (empty for leaf nodes).
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
}

/// Metadata for a leaf entity.
///
/// Minimal metadata needed for CRDT merge during sync.
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

/// CRDT type indicator for merge semantics.
///
/// Determines how entities are merged during sync.
#[derive(Clone, Copy, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub enum CrdtType {
    /// Last-Writer-Wins register (simple overwrite by HLC).
    LwwRegister,
    /// Grow-only counter.
    GCounter,
    /// Positive-negative counter.
    PnCounter,
    /// Last-Writer-Wins element set.
    LwwSet,
    /// Observed-Remove set.
    OrSet,
    /// Replicated Growable Array (ordered list).
    Rga,
    /// Unordered map with LWW values.
    UnorderedMap,
    /// Unordered set.
    UnorderedSet,
    /// Vector (ordered collection).
    Vector,
    /// Custom CRDT with named merge strategy.
    Custom(u32),
}

impl Default for CrdtType {
    fn default() -> Self {
        Self::LwwRegister
    }
}

/// Result of comparing two tree nodes.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeCompareResult {
    /// Hashes match - no sync needed for this subtree.
    Equal,
    /// Hashes differ - need to recurse or fetch leaf.
    Different {
        /// IDs of differing children to recurse into.
        differing_children: Vec<[u8; 32]>,
    },
    /// Local node missing - need to fetch from remote.
    LocalMissing,
    /// Remote node missing - nothing to sync.
    RemoteMissing,
}

impl TreeCompareResult {
    /// Check if sync is needed.
    #[must_use]
    pub fn needs_sync(&self) -> bool {
        !matches!(self, Self::Equal | Self::RemoteMissing)
    }
}

/// Compare local and remote tree nodes.
///
/// Returns which children (if any) need further traversal.
#[must_use]
pub fn compare_tree_nodes(local: Option<&TreeNode>, remote: &TreeNode) -> TreeCompareResult {
    match local {
        None => TreeCompareResult::LocalMissing,
        Some(local_node) => {
            if local_node.hash == remote.hash {
                TreeCompareResult::Equal
            } else {
                // Find differing children
                let differing: Vec<[u8; 32]> = remote
                    .children
                    .iter()
                    .filter(|child_id| {
                        // Child differs if not in local or hash differs
                        !local_node.children.contains(child_id)
                    })
                    .copied()
                    .collect();

                TreeCompareResult::Different {
                    differing_children: differing,
                }
            }
        }
    }
}

// =============================================================================
// BloomFilter Sync Types (CIP Appendix B - Protocol Selection Matrix)
// =============================================================================

/// Default false positive rate for Bloom filters.
pub const DEFAULT_BLOOM_FP_RATE: f32 = 0.01; // 1%

/// Minimum bits per element for reasonable FP rate.
const MIN_BITS_PER_ELEMENT: usize = 8;

/// FNV-1a 64-bit offset basis.
const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;

/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x100000001b3;

/// A Bloom filter for delta/entity ID membership testing.
///
/// CRITICAL: Uses FNV-1a hash for consistency across nodes.
/// POC Bug 5: Hash mismatch when one node used SipHash.
///
/// Use this for sync when:
/// - entity_count > 50
/// - divergence < 10%
/// - Want to minimize round trips (O(1) diff detection)
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct DeltaIdBloomFilter {
    /// Bit array (packed as bytes).
    bits: Vec<u8>,
    /// Number of bits in the filter.
    num_bits: usize,
    /// Number of hash functions to use.
    num_hashes: u8,
    /// Number of items inserted.
    item_count: usize,
}

impl DeltaIdBloomFilter {
    /// Create a new Bloom filter sized for expected items and FP rate.
    ///
    /// # Arguments
    /// * `expected_items` - Expected number of items to insert
    /// * `fp_rate` - Desired false positive rate (0.0 to 1.0)
    #[must_use]
    pub fn new(expected_items: usize, fp_rate: f32) -> Self {
        // Calculate optimal number of bits: m = -n * ln(p) / (ln(2)^2)
        let ln2_sq = std::f64::consts::LN_2 * std::f64::consts::LN_2;
        let num_bits = if expected_items == 0 {
            64 // Minimum size
        } else {
            let m = -(expected_items as f64) * (fp_rate as f64).ln() / ln2_sq;
            (m.ceil() as usize).max(expected_items * MIN_BITS_PER_ELEMENT)
        };

        // Calculate optimal number of hashes: k = (m/n) * ln(2)
        let num_hashes = if expected_items == 0 {
            4
        } else {
            let k = (num_bits as f64 / expected_items as f64) * std::f64::consts::LN_2;
            (k.ceil() as u8).clamp(1, 16)
        };

        let num_bytes = (num_bits + 7) / 8;

        Self {
            bits: vec![0; num_bytes],
            num_bits,
            num_hashes,
            item_count: 0,
        }
    }

    /// Create a filter with explicit parameters.
    #[must_use]
    pub fn with_params(num_bits: usize, num_hashes: u8) -> Self {
        let num_bytes = (num_bits + 7) / 8;
        Self {
            bits: vec![0; num_bytes],
            num_bits,
            num_hashes,
            item_count: 0,
        }
    }

    /// FNV-1a hash function.
    ///
    /// CRITICAL: This MUST be used by all nodes for consistency.
    /// Do NOT use DefaultHasher (SipHash) or other hash functions.
    #[must_use]
    pub fn hash_fnv1a(data: &[u8]) -> u64 {
        let mut hash: u64 = FNV_OFFSET_BASIS;
        for byte in data {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash
    }

    /// Compute hash positions using double hashing technique.
    fn compute_positions(&self, id: &[u8; 32]) -> Vec<usize> {
        let h1 = Self::hash_fnv1a(id);
        let h2 = Self::hash_fnv1a(&[id.as_slice(), &[0xFF]].concat());

        (0..self.num_hashes as u64)
            .map(|i| {
                let combined = h1.wrapping_add(i.wrapping_mul(h2));
                (combined as usize) % self.num_bits
            })
            .collect()
    }

    /// Insert an ID into the filter.
    pub fn insert(&mut self, id: &[u8; 32]) {
        let positions = self.compute_positions(id);
        for pos in positions {
            let byte_idx = pos / 8;
            let bit_idx = pos % 8;
            self.bits[byte_idx] |= 1 << bit_idx;
        }
        self.item_count += 1;
    }

    /// Check if an ID might be in the filter.
    ///
    /// Returns `true` if the ID is possibly in the set (may be false positive).
    /// Returns `false` if the ID is definitely not in the set.
    #[must_use]
    pub fn contains(&self, id: &[u8; 32]) -> bool {
        let positions = self.compute_positions(id);
        for pos in positions {
            let byte_idx = pos / 8;
            let bit_idx = pos % 8;
            if self.bits[byte_idx] & (1 << bit_idx) == 0 {
                return false;
            }
        }
        true
    }

    /// Get the number of items inserted.
    #[must_use]
    pub fn item_count(&self) -> usize {
        self.item_count
    }

    /// Get the filter size in bits.
    #[must_use]
    pub fn bit_count(&self) -> usize {
        self.num_bits
    }

    /// Get the number of hash functions.
    #[must_use]
    pub fn hash_count(&self) -> u8 {
        self.num_hashes
    }

    /// Estimate current false positive rate.
    #[must_use]
    pub fn estimated_fp_rate(&self) -> f64 {
        if self.item_count == 0 {
            return 0.0;
        }
        // FP rate ≈ (1 - e^(-k*n/m))^k
        let k = self.num_hashes as f64;
        let n = self.item_count as f64;
        let m = self.num_bits as f64;
        (1.0 - (-k * n / m).exp()).powf(k)
    }

    /// Get the raw bits (for serialization/debugging).
    #[must_use]
    pub fn bits(&self) -> &[u8] {
        &self.bits
    }
}

/// Request for Bloom filter-based sync.
///
/// Initiator sends their Bloom filter of known entity IDs.
/// Responder returns entities not in the filter.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct BloomFilterRequest {
    /// Bloom filter containing initiator's entity IDs.
    pub filter: DeltaIdBloomFilter,

    /// False positive rate used to build the filter.
    pub false_positive_rate: f32,
}

impl BloomFilterRequest {
    /// Create a new Bloom filter request.
    #[must_use]
    pub fn new(filter: DeltaIdBloomFilter, false_positive_rate: f32) -> Self {
        Self {
            filter,
            false_positive_rate,
        }
    }

    /// Create a request by building a filter from entity IDs.
    #[must_use]
    pub fn from_ids(ids: &[[u8; 32]], fp_rate: f32) -> Self {
        let mut filter = DeltaIdBloomFilter::new(ids.len(), fp_rate);
        for id in ids {
            filter.insert(id);
        }
        Self::new(filter, fp_rate)
    }
}

/// Response to a Bloom filter sync request.
///
/// Contains entities that the responder has but were not in the filter.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct BloomFilterResponse {
    /// Entities missing from the initiator.
    /// Includes full data and metadata for CRDT merge.
    pub missing_entities: Vec<TreeLeafData>,

    /// Number of entities scanned.
    pub scanned_count: usize,
}

impl BloomFilterResponse {
    /// Create a new response.
    #[must_use]
    pub fn new(missing_entities: Vec<TreeLeafData>, scanned_count: usize) -> Self {
        Self {
            missing_entities,
            scanned_count,
        }
    }

    /// Create an empty response (no missing entities).
    #[must_use]
    pub fn empty(scanned_count: usize) -> Self {
        Self {
            missing_entities: vec![],
            scanned_count,
        }
    }

    /// Check if there are missing entities.
    #[must_use]
    pub fn has_missing(&self) -> bool {
        !self.missing_entities.is_empty()
    }

    /// Get count of missing entities.
    #[must_use]
    pub fn missing_count(&self) -> usize {
        self.missing_entities.len()
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
    // Delta Buffering Tests (Issue #1773)
    // =========================================================================

    #[test]
    fn test_sync_state_transitions() {
        assert!(!SyncState::Idle.is_active());
        assert!(!SyncState::Idle.should_buffer_deltas());

        assert!(SyncState::Handshaking.is_active());
        assert!(!SyncState::Handshaking.should_buffer_deltas());

        assert!(SyncState::ReceivingState.is_active());
        assert!(SyncState::ReceivingState.should_buffer_deltas());

        assert!(SyncState::ReplayingDeltas.is_active());
        assert!(!SyncState::ReplayingDeltas.should_buffer_deltas());

        assert!(!SyncState::Completed.is_active());
        assert!(!SyncState::Failed("test".to_string()).is_active());
    }

    #[test]
    fn test_buffered_delta_roundtrip() {
        let delta = BufferedDelta::new(
            [1; 32],
            vec![[2; 32], [3; 32]],
            12345,
            [4; 24],
            [5; 32],
            [6; 32],
            vec![7, 8, 9],
            vec![vec![10, 11], vec![12, 13]],
        );

        let encoded = borsh::to_vec(&delta).expect("serialize");
        let decoded: BufferedDelta = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(delta, decoded);
        assert!(!decoded.is_genesis());
    }

    #[test]
    fn test_buffered_delta_genesis() {
        let genesis = BufferedDelta::new(
            [1; 32],
            vec![], // No parents
            0,
            [0; 24],
            [2; 32],
            [3; 32],
            vec![1, 2, 3],
            vec![],
        );
        assert!(genesis.is_genesis());

        let non_genesis = BufferedDelta::new(
            [2; 32],
            vec![[1; 32]], // Has parent
            1,
            [0; 24],
            [2; 32],
            [4; 32],
            vec![4, 5, 6],
            vec![],
        );
        assert!(!non_genesis.is_genesis());
    }

    #[test]
    fn test_sync_context_lifecycle() {
        let mut ctx = SyncContext::new();
        assert!(!ctx.is_active());
        assert!(!ctx.should_buffer());
        assert_eq!(ctx.buffered_count(), 0);

        // Start sync
        ctx.start(
            [1; 32],
            SyncProtocol::HashComparison {
                root_hash: [2; 32],
                divergent_subtrees: vec![],
            },
            1000,
        );
        assert!(ctx.is_active());
        assert!(!ctx.should_buffer()); // Handshaking, not yet receiving

        // Begin receiving state
        ctx.begin_receiving();
        assert!(ctx.is_active());
        assert!(ctx.should_buffer());

        // Begin replay
        ctx.begin_replay();
        assert!(ctx.is_active());
        assert!(!ctx.should_buffer());

        // Complete
        ctx.complete();
        assert!(!ctx.is_active());
    }

    #[test]
    fn test_sync_context_buffer_deltas() {
        let mut ctx = SyncContext::with_capacity(3);
        ctx.begin_receiving();

        let delta1 = BufferedDelta::new(
            [1; 32],
            vec![],
            1,
            [0; 24],
            [0; 32],
            [0; 32],
            vec![],
            vec![],
        );
        let delta2 = BufferedDelta::new(
            [2; 32],
            vec![[1; 32]],
            2,
            [0; 24],
            [0; 32],
            [0; 32],
            vec![],
            vec![],
        );
        let delta3 = BufferedDelta::new(
            [3; 32],
            vec![[2; 32]],
            3,
            [0; 24],
            [0; 32],
            [0; 32],
            vec![],
            vec![],
        );
        let delta4 = BufferedDelta::new(
            [4; 32],
            vec![[3; 32]],
            4,
            [0; 24],
            [0; 32],
            [0; 32],
            vec![],
            vec![],
        );

        // Buffer within capacity
        assert!(ctx.buffer_delta(delta1));
        assert!(ctx.buffer_delta(delta2));
        assert!(ctx.buffer_delta(delta3));
        assert_eq!(ctx.buffered_count(), 3);
        assert!(!ctx.is_buffer_exceeded());

        // Buffer exceeds capacity (but still buffers - I6)
        assert!(!ctx.buffer_delta(delta4)); // Returns false - exceeded
        assert_eq!(ctx.buffered_count(), 4); // Delta is still buffered!
        assert!(ctx.is_buffer_exceeded());
    }

    #[test]
    fn test_sync_context_take_buffered() {
        let mut ctx = SyncContext::new();
        ctx.begin_receiving();

        let delta1 = BufferedDelta::new(
            [1; 32],
            vec![],
            1,
            [0; 24],
            [0; 32],
            [0; 32],
            vec![],
            vec![],
        );
        let delta2 = BufferedDelta::new(
            [2; 32],
            vec![[1; 32]],
            2,
            [0; 24],
            [0; 32],
            [0; 32],
            vec![],
            vec![],
        );

        ctx.buffer_delta(delta1.clone());
        ctx.buffer_delta(delta2.clone());
        assert_eq!(ctx.buffered_count(), 2);

        // Take deltas for replay
        let taken = ctx.take_buffered_deltas();
        assert_eq!(taken.len(), 2);
        assert_eq!(taken[0].id, delta1.id);
        assert_eq!(taken[1].id, delta2.id);

        // Buffer is now empty
        assert_eq!(ctx.buffered_count(), 0);
    }

    #[test]
    fn test_sync_context_reset() {
        let mut ctx = SyncContext::new();
        ctx.start(
            [1; 32],
            SyncProtocol::Snapshot {
                compressed: true,
                verified: true,
            },
            1000,
        );
        ctx.begin_receiving();

        let delta = BufferedDelta::new(
            [1; 32],
            vec![],
            1,
            [0; 24],
            [0; 32],
            [0; 32],
            vec![],
            vec![],
        );
        ctx.buffer_delta(delta);

        assert!(ctx.is_active());
        assert!(ctx.peer_id.is_some());
        assert_eq!(ctx.buffered_count(), 1);

        ctx.reset();

        assert!(!ctx.is_active());
        assert!(ctx.peer_id.is_none());
        assert_eq!(ctx.buffered_count(), 0);
        assert!(matches!(ctx.protocol, SyncProtocol::None));
    }

    #[test]
    fn test_sync_context_fail() {
        let mut ctx = SyncContext::new();
        ctx.start(
            [1; 32],
            SyncProtocol::HashComparison {
                root_hash: [0; 32],
                divergent_subtrees: vec![],
            },
            1000,
        );

        ctx.fail("connection lost".to_string());
        assert!(!ctx.is_active());
        assert!(matches!(ctx.state, SyncState::Failed(ref msg) if msg == "connection lost"));
    }

    #[test]
    fn test_buffer_metrics() {
        let mut metrics = BufferMetrics::default();

        metrics.record_buffer(1);
        metrics.record_buffer(2);
        metrics.record_buffer(3);
        assert_eq!(metrics.total_buffered, 3);
        assert_eq!(metrics.peak_buffer_size, 3);

        metrics.record_exceeded();
        metrics.record_exceeded();
        assert_eq!(metrics.capacity_exceeded_count, 2);

        metrics.record_replay(5);
        assert_eq!(metrics.total_replayed, 5);

        metrics.record_completion(1500);
        assert_eq!(metrics.sync_duration_ms, Some(1500));
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
        assert_eq!(decoded.leaves().len(), 1);
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
        let metadata = LeafMetadata::new(CrdtType::PnCounter, 500, [1; 32])
            .with_version(10)
            .with_parent([2; 32]);

        assert_eq!(metadata.crdt_type, CrdtType::PnCounter);
        assert_eq!(metadata.hlc_timestamp, 500);
        assert_eq!(metadata.version, 10);
        assert_eq!(metadata.parent_id, Some([2; 32]));
    }

    #[test]
    fn test_crdt_type_variants() {
        let types = vec![
            CrdtType::LwwRegister,
            CrdtType::GCounter,
            CrdtType::PnCounter,
            CrdtType::LwwSet,
            CrdtType::OrSet,
            CrdtType::Rga,
            CrdtType::UnorderedMap,
            CrdtType::UnorderedSet,
            CrdtType::Vector,
            CrdtType::Custom(42),
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

        let result = compare_tree_nodes(Some(&local), &remote);
        assert_eq!(result, TreeCompareResult::Equal);
        assert!(!result.needs_sync());
    }

    #[test]
    fn test_compare_tree_nodes_local_missing() {
        let remote = TreeNode::internal([1; 32], [2; 32], vec![[3; 32]]);

        let result = compare_tree_nodes(None, &remote);
        assert_eq!(result, TreeCompareResult::LocalMissing);
        assert!(result.needs_sync());
    }

    #[test]
    fn test_compare_tree_nodes_different() {
        let local = TreeNode::internal([1; 32], [10; 32], vec![[2; 32]]);
        let remote = TreeNode::internal([1; 32], [20; 32], vec![[2; 32], [3; 32]]);

        let result = compare_tree_nodes(Some(&local), &remote);

        match &result {
            TreeCompareResult::Different { differing_children } => {
                // [3; 32] is in remote but not in local
                assert!(differing_children.contains(&[3; 32]));
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
            differing_children: vec![]
        }
        .needs_sync());
    }

    // =========================================================================
    // BloomFilter Sync Tests (Issue #1775)
    // =========================================================================

    #[test]
    fn test_bloom_filter_fnv1a_consistency() {
        // FNV-1a must produce consistent results
        let data = [1u8; 32];
        let hash1 = DeltaIdBloomFilter::hash_fnv1a(&data);
        let hash2 = DeltaIdBloomFilter::hash_fnv1a(&data);
        assert_eq!(hash1, hash2);

        // Different data should (very likely) produce different hashes
        let other_data = [2u8; 32];
        let other_hash = DeltaIdBloomFilter::hash_fnv1a(&other_data);
        assert_ne!(hash1, other_hash);
    }

    #[test]
    fn test_bloom_filter_insert_contains() {
        let mut filter = DeltaIdBloomFilter::new(100, 0.01);

        let id1 = [1u8; 32];
        let id2 = [2u8; 32];
        let id3 = [3u8; 32];

        // Initially empty
        assert!(!filter.contains(&id1));
        assert!(!filter.contains(&id2));

        // Insert and check
        filter.insert(&id1);
        filter.insert(&id2);

        assert!(filter.contains(&id1));
        assert!(filter.contains(&id2));
        assert!(!filter.contains(&id3)); // Not inserted
    }

    #[test]
    fn test_bloom_filter_item_count() {
        let mut filter = DeltaIdBloomFilter::new(100, 0.01);
        assert_eq!(filter.item_count(), 0);

        filter.insert(&[1u8; 32]);
        assert_eq!(filter.item_count(), 1);

        filter.insert(&[2u8; 32]);
        filter.insert(&[3u8; 32]);
        assert_eq!(filter.item_count(), 3);
    }

    #[test]
    fn test_bloom_filter_roundtrip() {
        let mut filter = DeltaIdBloomFilter::new(50, 0.01);
        filter.insert(&[1u8; 32]);
        filter.insert(&[2u8; 32]);
        filter.insert(&[3u8; 32]);

        let encoded = borsh::to_vec(&filter).expect("serialize");
        let decoded: DeltaIdBloomFilter = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(filter, decoded);
        assert!(decoded.contains(&[1u8; 32]));
        assert!(decoded.contains(&[2u8; 32]));
        assert!(decoded.contains(&[3u8; 32]));
        assert!(!decoded.contains(&[4u8; 32]));
    }

    #[test]
    fn test_bloom_filter_false_positive_rate() {
        // Create a filter and fill it
        let num_items = 1000;
        let target_fp_rate = 0.01;
        let mut filter = DeltaIdBloomFilter::new(num_items, target_fp_rate);

        // Insert items
        for i in 0..num_items {
            let mut id = [0u8; 32];
            id[0..8].copy_from_slice(&(i as u64).to_le_bytes());
            filter.insert(&id);
        }

        // Test false positives with items not inserted
        let test_count = 10000;
        let mut false_positives = 0;
        for i in num_items..(num_items + test_count) {
            let mut id = [0u8; 32];
            id[0..8].copy_from_slice(&(i as u64).to_le_bytes());
            if filter.contains(&id) {
                false_positives += 1;
            }
        }

        let actual_fp_rate = false_positives as f64 / test_count as f64;
        // Allow some tolerance (FP rate should be roughly in the right ballpark)
        assert!(
            actual_fp_rate < target_fp_rate as f64 * 3.0,
            "FP rate {} too high (target {})",
            actual_fp_rate,
            target_fp_rate
        );
    }

    #[test]
    fn test_bloom_filter_estimated_fp_rate() {
        let mut filter = DeltaIdBloomFilter::new(100, 0.01);

        // Empty filter has 0 FP rate
        assert_eq!(filter.estimated_fp_rate(), 0.0);

        // Fill partially
        for i in 0..50 {
            let mut id = [0u8; 32];
            id[0..8].copy_from_slice(&(i as u64).to_le_bytes());
            filter.insert(&id);
        }

        // Estimated FP rate should be positive but reasonable
        let estimated = filter.estimated_fp_rate();
        assert!(estimated > 0.0);
        assert!(estimated < 0.1); // Should be well under 10% with 50% fill
    }

    #[test]
    fn test_bloom_filter_request_from_ids() {
        let ids = [[1u8; 32], [2u8; 32], [3u8; 32]];
        let request = BloomFilterRequest::from_ids(&ids, 0.01);

        assert!(request.filter.contains(&[1u8; 32]));
        assert!(request.filter.contains(&[2u8; 32]));
        assert!(request.filter.contains(&[3u8; 32]));
        assert!(!request.filter.contains(&[4u8; 32]));
        assert_eq!(request.false_positive_rate, 0.01);
    }

    #[test]
    fn test_bloom_filter_request_roundtrip() {
        let ids = [[1u8; 32], [2u8; 32]];
        let request = BloomFilterRequest::from_ids(&ids, 0.02);

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: BloomFilterRequest = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(request, decoded);
    }

    #[test]
    fn test_bloom_filter_response() {
        let metadata = LeafMetadata::new(CrdtType::LwwRegister, 100, [5; 32]);
        let leaf = TreeLeafData::new([1; 32], vec![1, 2, 3], metadata);

        let response = BloomFilterResponse::new(vec![leaf.clone()], 100);

        assert!(response.has_missing());
        assert_eq!(response.missing_count(), 1);
        assert_eq!(response.scanned_count, 100);
    }

    #[test]
    fn test_bloom_filter_response_empty() {
        let response = BloomFilterResponse::empty(50);

        assert!(!response.has_missing());
        assert_eq!(response.missing_count(), 0);
        assert_eq!(response.scanned_count, 50);
    }

    #[test]
    fn test_bloom_filter_response_roundtrip() {
        let metadata = LeafMetadata::new(CrdtType::UnorderedMap, 200, [6; 32]);
        let leaf = TreeLeafData::new([2; 32], vec![4, 5, 6], metadata);

        let response = BloomFilterResponse::new(vec![leaf], 75);

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: BloomFilterResponse = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(response, decoded);
    }

    #[test]
    fn test_bloom_filter_with_params() {
        let filter = DeltaIdBloomFilter::with_params(1024, 7);

        assert_eq!(filter.bit_count(), 1024);
        assert_eq!(filter.hash_count(), 7);
        assert_eq!(filter.item_count(), 0);
    }
}
