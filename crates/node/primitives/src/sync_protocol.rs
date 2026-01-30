//! Sync protocol types and abstractions for network synchronization.
//!
//! This module defines the protocol negotiation, sync hints, and
//! merge callback abstractions used during state synchronization.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::hash::Hash;

// ============================================================================
// Protocol Negotiation
// ============================================================================

/// Supported sync protocols with version information.
///
/// Used during handshake to negotiate which sync protocol to use.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum SyncProtocolVersion {
    /// Delta-based sync (DAG catchup)
    DeltaSync { version: u8 },
    /// Full snapshot transfer
    SnapshotSync { version: u8 },
    /// Hybrid: snapshot + delta fine-sync
    HybridSync { version: u8 },
}

impl Default for SyncProtocolVersion {
    fn default() -> Self {
        Self::DeltaSync { version: 1 }
    }
}

/// Capabilities advertised during handshake.
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SyncCapabilities {
    /// Protocols this node supports, in preference order.
    pub supported_protocols: Vec<SyncProtocolVersion>,
    /// Maximum snapshot page size this node can handle.
    pub max_page_size: u32,
    /// Whether this node supports compressed snapshots.
    pub supports_compression: bool,
    /// Whether this node supports sync hints in deltas.
    pub supports_sync_hints: bool,
}

impl SyncCapabilities {
    /// Create capabilities with all features enabled.
    #[must_use]
    pub fn full() -> Self {
        Self {
            supported_protocols: vec![
                SyncProtocolVersion::HybridSync { version: 1 },
                SyncProtocolVersion::SnapshotSync { version: 1 },
                SyncProtocolVersion::DeltaSync { version: 1 },
            ],
            max_page_size: 1024 * 1024, // 1 MiB
            supports_compression: true,
            supports_sync_hints: true,
        }
    }

    /// Create minimal capabilities (delta sync only).
    #[must_use]
    pub fn minimal() -> Self {
        Self {
            supported_protocols: vec![SyncProtocolVersion::DeltaSync { version: 1 }],
            max_page_size: 64 * 1024, // 64 KiB
            supports_compression: false,
            supports_sync_hints: false,
        }
    }

    /// Negotiate common protocol between two capability sets.
    #[must_use]
    pub fn negotiate(&self, peer: &Self) -> Option<SyncProtocolVersion> {
        // Find first protocol we support that peer also supports
        for our_proto in &self.supported_protocols {
            for peer_proto in &peer.supported_protocols {
                if Self::protocols_compatible(our_proto, peer_proto) {
                    return Some(our_proto.clone());
                }
            }
        }
        None
    }

    fn protocols_compatible(a: &SyncProtocolVersion, b: &SyncProtocolVersion) -> bool {
        match (a, b) {
            (
                SyncProtocolVersion::DeltaSync { version: v1 },
                SyncProtocolVersion::DeltaSync { version: v2 },
            ) => v1 == v2,
            (
                SyncProtocolVersion::SnapshotSync { version: v1 },
                SyncProtocolVersion::SnapshotSync { version: v2 },
            ) => v1 == v2,
            (
                SyncProtocolVersion::HybridSync { version: v1 },
                SyncProtocolVersion::HybridSync { version: v2 },
            ) => v1 == v2,
            _ => false,
        }
    }
}

// ============================================================================
// Gossip Mode
// ============================================================================

/// Mode for delta gossip propagation.
///
/// Controls whether sync hints are included with delta broadcasts.
/// This allows trading off between bandwidth and sync responsiveness.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum GossipMode {
    /// Include sync hints with every delta (~40 bytes overhead).
    ///
    /// Enables:
    /// - Proactive divergence detection
    /// - Adaptive protocol selection by receivers
    /// - Faster recovery from network partitions
    #[default]
    WithHints,

    /// Send deltas without sync hints (minimal bandwidth).
    ///
    /// Use when:
    /// - Network is bandwidth-constrained
    /// - All nodes are well-synced (heartbeats sufficient)
    /// - Testing or debugging without hint complexity
    Minimal,

    /// Adaptive mode: include hints only when divergence is likely.
    ///
    /// Triggers hints when:
    /// - Entity count changed significantly (>10% delta)
    /// - Tree depth increased
    /// - After sync completion (announce new state)
    Adaptive {
        /// Minimum entity count change to trigger hints.
        entity_change_threshold: u32,
    },
}

impl GossipMode {
    /// Create adaptive mode with default thresholds.
    #[must_use]
    pub fn adaptive() -> Self {
        Self::Adaptive {
            entity_change_threshold: 10,
        }
    }

    /// Check if hints should be included for a state change.
    #[must_use]
    pub fn should_include_hints(&self, entity_count_delta: i32) -> bool {
        match self {
            Self::WithHints => true,
            Self::Minimal => false,
            Self::Adaptive {
                entity_change_threshold,
            } => entity_count_delta.unsigned_abs() >= *entity_change_threshold,
        }
    }

    /// Create hints based on mode and state.
    ///
    /// Returns `Some(SyncHints)` if hints should be included, `None` otherwise.
    #[must_use]
    pub fn create_hints(
        &self,
        root_hash: Hash,
        entity_count: u32,
        tree_depth: u8,
        entity_count_delta: i32,
    ) -> Option<SyncHints> {
        if self.should_include_hints(entity_count_delta) {
            Some(SyncHints::from_state(root_hash, entity_count, tree_depth))
        } else {
            // Return minimal hints with just the hash
            // (required field, but receiver knows hints aren't authoritative)
            Some(SyncHints {
                post_root_hash: root_hash,
                entity_count: 0,
                tree_depth: 0,
                suggested_protocol: SyncProtocolHint::DeltaSync,
            })
        }
    }
}

/// Handshake message for protocol negotiation.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SyncHandshake {
    /// Our capabilities.
    pub capabilities: SyncCapabilities,
    /// Our current root hash.
    pub root_hash: Hash,
    /// Our current DAG heads.
    pub dag_heads: Vec<[u8; 32]>,
    /// Entity count (for divergence estimation).
    pub entity_count: u64,
}

/// Response to handshake with negotiated protocol.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SyncHandshakeResponse {
    /// Negotiated protocol (None if no common protocol).
    pub negotiated_protocol: Option<SyncProtocolVersion>,
    /// Peer's capabilities for reference.
    pub capabilities: SyncCapabilities,
    /// Peer's current root hash.
    pub root_hash: Hash,
    /// Peer's current DAG heads.
    pub dag_heads: Vec<[u8; 32]>,
    /// Peer's entity count.
    pub entity_count: u64,
}

// ============================================================================
// Sync Hints
// ============================================================================

/// Lightweight hints included in delta messages for proactive sync.
///
/// These hints allow receiving nodes to detect divergence early
/// and trigger sync without waiting for periodic checks.
///
/// Total overhead: ~40 bytes per delta message.
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SyncHints {
    /// Root hash after applying this delta.
    pub post_root_hash: Hash,
    /// Number of entities in the tree (for divergence estimation).
    pub entity_count: u32,
    /// Depth of the Merkle tree (for protocol selection).
    pub tree_depth: u8,
    /// Hint about expected sync protocol if divergent.
    pub suggested_protocol: SyncProtocolHint,
}

impl SyncHints {
    /// Create sync hints from current state.
    #[must_use]
    pub fn from_state(root_hash: Hash, entity_count: u32, tree_depth: u8) -> Self {
        let suggested_protocol = Self::suggest_protocol(entity_count, tree_depth);
        Self {
            post_root_hash: root_hash,
            entity_count,
            tree_depth,
            suggested_protocol,
        }
    }

    /// Suggest optimal sync protocol based on state characteristics.
    fn suggest_protocol(entity_count: u32, tree_depth: u8) -> SyncProtocolHint {
        // Heuristics for protocol selection:
        // - Small trees (<100 entities): Delta sync is usually sufficient
        // - Medium trees (100-10000 entities): Hash-based comparison
        // - Large trees (>10000 entities): Consider snapshot for large divergence
        if entity_count < 100 {
            SyncProtocolHint::DeltaSync
        } else if entity_count < 10000 || tree_depth < 5 {
            SyncProtocolHint::HashBased
        } else {
            SyncProtocolHint::AdaptiveSelection
        }
    }

    /// Check if these hints suggest divergence from local state.
    #[must_use]
    pub fn suggests_divergence(&self, local_root_hash: &Hash, local_entity_count: u32) -> bool {
        // Divergence if root hashes differ
        if self.post_root_hash != *local_root_hash {
            return true;
        }
        // Large entity count difference suggests partial sync needed
        let count_diff = (self.entity_count as i64 - local_entity_count as i64).abs();
        count_diff > 10 // Threshold for significant divergence
    }

    /// Perform adaptive protocol selection based on local state.
    ///
    /// When `suggested_protocol` is `AdaptiveSelection`, the receiver uses
    /// their local state to decide the best sync approach.
    ///
    /// # Decision Logic
    ///
    /// ```text
    /// 1. No divergence (same hash) → None (no sync needed)
    /// 2. Local is empty → Snapshot (bootstrap)
    /// 3. Sender has 10x+ more entities → Snapshot (we're far behind)
    /// 4. Small local tree (<100 entities) → DeltaSync
    /// 5. Medium local tree (100-10000) → HashBased
    /// 6. Large local tree (>10000) → HashBased (still better than snapshot)
    /// ```
    #[must_use]
    pub fn adaptive_select(
        &self,
        local_root_hash: &Hash,
        local_entity_count: u32,
    ) -> Option<SyncProtocolHint> {
        // No divergence - no sync needed
        if self.post_root_hash == *local_root_hash {
            return None;
        }

        // Local is empty - need full bootstrap
        if local_entity_count == 0 {
            return Some(SyncProtocolHint::Snapshot);
        }

        // Sender has significantly more entities (10x+) - we're far behind
        if self.entity_count > local_entity_count.saturating_mul(10) {
            return Some(SyncProtocolHint::Snapshot);
        }

        // Choose based on local tree size
        if local_entity_count < 100 {
            // Small tree - delta sync can handle it
            Some(SyncProtocolHint::DeltaSync)
        } else if local_entity_count < 10000 {
            // Medium tree - hash-based comparison is efficient
            Some(SyncProtocolHint::HashBased)
        } else {
            // Large tree - still prefer hash-based over snapshot
            // (snapshot is expensive, hash-based finds specific differences)
            Some(SyncProtocolHint::HashBased)
        }
    }
}

/// Hint about which sync protocol might be optimal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum SyncProtocolHint {
    /// Delta sync should be sufficient.
    #[default]
    DeltaSync,
    /// Hash-based tree comparison recommended.
    HashBased,
    /// Full snapshot may be needed.
    Snapshot,
    /// Let the receiver decide based on local state.
    AdaptiveSelection,
}

// ============================================================================
// Sync State Machine
// ============================================================================

/// Current state of a sync session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncSessionState {
    /// Initial state, no sync in progress.
    Idle,
    /// Handshake sent, waiting for response.
    Handshaking,
    /// Protocol negotiated, sync in progress.
    Syncing {
        protocol: SyncProtocolVersion,
        started_at: u64,
    },
    /// Delta buffering during snapshot sync.
    BufferingDeltas {
        buffered_count: usize,
        sync_start_hlc: u64,
    },
    /// Replaying buffered deltas after snapshot.
    ReplayingDeltas { remaining: usize },
    /// Sync completed successfully.
    Completed {
        protocol: SyncProtocolVersion,
        duration_ms: u64,
    },
    /// Sync failed.
    Failed { reason: String },
}

impl Default for SyncSessionState {
    fn default() -> Self {
        Self::Idle
    }
}

impl SyncSessionState {
    /// Check if sync is currently in progress.
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Handshaking
                | Self::Syncing { .. }
                | Self::BufferingDeltas { .. }
                | Self::ReplayingDeltas { .. }
        )
    }

    /// Check if deltas should be buffered (during snapshot sync).
    #[must_use]
    pub fn should_buffer_deltas(&self) -> bool {
        matches!(self, Self::BufferingDeltas { .. })
    }
}

// ============================================================================
// Delta Buffer for Sync
// ============================================================================

/// Buffer for deltas received during snapshot sync.
///
/// Deltas are stored and replayed after snapshot application.
#[derive(Debug, Default)]
pub struct DeltaBuffer {
    /// Buffered deltas in order received.
    deltas: Vec<BufferedDelta>,
    /// HLC timestamp when buffering started.
    sync_start_hlc: u64,
    /// Maximum buffer size before forcing snapshot restart.
    max_size: usize,
}

/// A single buffered delta.
#[derive(Debug, Clone)]
pub struct BufferedDelta {
    /// Delta ID.
    pub id: [u8; 32],
    /// Parent IDs.
    pub parents: Vec<[u8; 32]>,
    /// HLC timestamp.
    pub hlc: u64,
    /// Serialized payload.
    pub payload: Vec<u8>,
}

impl DeltaBuffer {
    /// Create a new delta buffer with specified capacity.
    #[must_use]
    pub fn new(max_size: usize, sync_start_hlc: u64) -> Self {
        Self {
            deltas: Vec::with_capacity(max_size.min(1000)),
            sync_start_hlc,
            max_size,
        }
    }

    /// Add a delta to the buffer.
    ///
    /// Returns `Err` if buffer is full and sync should restart.
    pub fn push(&mut self, delta: BufferedDelta) -> Result<(), DeltaBufferFull> {
        if self.deltas.len() >= self.max_size {
            return Err(DeltaBufferFull {
                buffered_count: self.deltas.len(),
            });
        }
        self.deltas.push(delta);
        Ok(())
    }

    /// Get all buffered deltas for replay.
    #[must_use]
    pub fn drain(&mut self) -> Vec<BufferedDelta> {
        std::mem::take(&mut self.deltas)
    }

    /// Number of buffered deltas.
    #[must_use]
    pub fn len(&self) -> usize {
        self.deltas.len()
    }

    /// Check if buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.deltas.is_empty()
    }

    /// Get the sync start HLC.
    #[must_use]
    pub fn sync_start_hlc(&self) -> u64 {
        self.sync_start_hlc
    }
}

/// Error when delta buffer is full.
#[derive(Debug, Clone)]
pub struct DeltaBufferFull {
    /// Number of deltas already buffered.
    pub buffered_count: usize,
}

impl std::fmt::Display for DeltaBufferFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Delta buffer full ({} deltas), sync should restart",
            self.buffered_count
        )
    }
}

impl std::error::Error for DeltaBufferFull {}

// ============================================================================
// Delta ID Bloom Filter
// ============================================================================

/// Bloom filter for efficient delta ID membership testing.
///
/// Used to quickly check "do you have these deltas?" without transferring
/// full ID lists. False positives are possible but false negatives are not.
///
/// # Usage
///
/// ```ignore
/// let mut filter = DeltaIdBloomFilter::with_capacity(1000, 0.01);
/// filter.insert(&delta_id);
/// if filter.maybe_contains(&other_id) {
///     // Might have it - verify with actual lookup
/// }
/// ```
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct DeltaIdBloomFilter {
    /// Bit array storage.
    bits: Vec<u8>,
    /// Number of hash functions.
    num_hashes: u8,
    /// Number of items inserted.
    num_items: u32,
}

impl DeltaIdBloomFilter {
    /// Create a new bloom filter with given capacity and false positive rate.
    ///
    /// # Arguments
    /// * `expected_items` - Expected number of delta IDs to store
    /// * `false_positive_rate` - Desired false positive rate (e.g., 0.01 for 1%)
    #[must_use]
    pub fn with_capacity(expected_items: usize, false_positive_rate: f64) -> Self {
        // Calculate optimal size: m = -n * ln(p) / (ln(2)^2)
        let n = expected_items.max(1) as f64;
        let p = false_positive_rate.max(0.0001).min(0.5);
        let m = (-n * p.ln() / (2_f64.ln().powi(2))).ceil() as usize;
        let m = m.max(64); // Minimum 64 bits

        // Calculate optimal hash count: k = m/n * ln(2)
        let k = ((m as f64 / n) * 2_f64.ln()).ceil() as usize;
        let k = k.clamp(1, 16) as u8;

        Self {
            bits: vec![0; (m + 7) / 8],
            num_hashes: k,
            num_items: 0,
        }
    }

    /// Create a filter optimized for typical delta sync scenarios.
    ///
    /// Uses 1% false positive rate with capacity for 1000 deltas.
    #[must_use]
    pub fn default_for_sync() -> Self {
        Self::with_capacity(1000, 0.01)
    }

    /// Insert a delta ID into the filter.
    pub fn insert(&mut self, delta_id: &[u8; 32]) {
        for i in 0..self.num_hashes {
            let hash = self.hash(delta_id, i);
            let bit_index = hash % (self.bits.len() * 8);
            self.bits[bit_index / 8] |= 1 << (bit_index % 8);
        }
        self.num_items += 1;
    }

    /// Check if a delta ID might be in the filter.
    ///
    /// Returns `true` if possibly present, `false` if definitely absent.
    #[must_use]
    pub fn maybe_contains(&self, delta_id: &[u8; 32]) -> bool {
        for i in 0..self.num_hashes {
            let hash = self.hash(delta_id, i);
            let bit_index = hash % (self.bits.len() * 8);
            if self.bits[bit_index / 8] & (1 << (bit_index % 8)) == 0 {
                return false;
            }
        }
        true
    }

    /// Get the number of items inserted.
    #[must_use]
    pub fn len(&self) -> usize {
        self.num_items as usize
    }

    /// Check if the filter is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.num_items == 0
    }

    /// Get the size of the filter in bytes.
    #[must_use]
    pub fn size_bytes(&self) -> usize {
        self.bits.len()
    }

    /// Get the estimated false positive rate for current fill level.
    #[must_use]
    pub fn estimated_fp_rate(&self) -> f64 {
        let m = (self.bits.len() * 8) as f64;
        let k = self.num_hashes as f64;
        let n = self.num_items as f64;
        (1.0 - (-k * n / m).exp()).powf(k)
    }

    /// Hash function using FNV-1a with seed.
    fn hash(&self, data: &[u8; 32], seed: u8) -> usize {
        let mut hash: u64 = 0xcbf29ce484222325_u64; // FNV offset basis
        hash = hash.wrapping_add(seed as u64);
        for byte in data {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3); // FNV prime
        }
        hash as usize
    }

    /// Find delta IDs from a list that are definitely NOT in this filter.
    ///
    /// Returns IDs that the filter owner definitely doesn't have.
    /// This is useful for sync: ask "which of these do you need?"
    #[must_use]
    pub fn filter_missing(&self, ids: &[[u8; 32]]) -> Vec<[u8; 32]> {
        ids.iter()
            .filter(|id| !self.maybe_contains(id))
            .copied()
            .collect()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_negotiation_full_match() {
        let caps_a = SyncCapabilities::full();
        let caps_b = SyncCapabilities::full();

        let negotiated = caps_a.negotiate(&caps_b);
        assert!(negotiated.is_some());
        assert!(matches!(
            negotiated.unwrap(),
            SyncProtocolVersion::HybridSync { version: 1 }
        ));
    }

    #[test]
    fn test_capability_negotiation_minimal() {
        let caps_full = SyncCapabilities::full();
        let caps_minimal = SyncCapabilities::minimal();

        // Full node negotiating with minimal node
        let negotiated = caps_full.negotiate(&caps_minimal);
        assert!(negotiated.is_some());
        assert!(matches!(
            negotiated.unwrap(),
            SyncProtocolVersion::DeltaSync { version: 1 }
        ));
    }

    #[test]
    fn test_capability_negotiation_no_match() {
        let caps_a = SyncCapabilities {
            supported_protocols: vec![SyncProtocolVersion::HybridSync { version: 2 }],
            ..Default::default()
        };
        let caps_b = SyncCapabilities {
            supported_protocols: vec![SyncProtocolVersion::DeltaSync { version: 1 }],
            ..Default::default()
        };

        let negotiated = caps_a.negotiate(&caps_b);
        assert!(negotiated.is_none());
    }

    #[test]
    fn test_sync_hints_divergence_detection() {
        let hints = SyncHints::from_state(Hash::from([1; 32]), 100, 5);

        // Same root hash, similar entity count - no divergence
        assert!(!hints.suggests_divergence(&Hash::from([1; 32]), 105));

        // Different root hash - divergence
        assert!(hints.suggests_divergence(&Hash::from([2; 32]), 100));

        // Large entity count difference - divergence
        assert!(hints.suggests_divergence(&Hash::from([1; 32]), 50));
    }

    #[test]
    fn test_sync_hints_protocol_suggestion() {
        // Small tree
        let hints_small = SyncHints::from_state(Hash::from([1; 32]), 50, 3);
        assert_eq!(hints_small.suggested_protocol, SyncProtocolHint::DeltaSync);

        // Medium tree
        let hints_medium = SyncHints::from_state(Hash::from([1; 32]), 500, 6);
        assert_eq!(hints_medium.suggested_protocol, SyncProtocolHint::HashBased);

        // Large tree
        let hints_large = SyncHints::from_state(Hash::from([1; 32]), 50000, 10);
        assert_eq!(
            hints_large.suggested_protocol,
            SyncProtocolHint::AdaptiveSelection
        );
    }

    #[test]
    fn test_sync_session_state_transitions() {
        let state = SyncSessionState::Idle;
        assert!(!state.is_active());
        assert!(!state.should_buffer_deltas());

        let state = SyncSessionState::Syncing {
            protocol: SyncProtocolVersion::DeltaSync { version: 1 },
            started_at: 12345,
        };
        assert!(state.is_active());
        assert!(!state.should_buffer_deltas());

        let state = SyncSessionState::BufferingDeltas {
            buffered_count: 10,
            sync_start_hlc: 12345,
        };
        assert!(state.is_active());
        assert!(state.should_buffer_deltas());
    }

    #[test]
    fn test_delta_buffer_basic() {
        let mut buffer = DeltaBuffer::new(100, 12345);
        assert!(buffer.is_empty());
        assert_eq!(buffer.sync_start_hlc(), 12345);

        let delta = BufferedDelta {
            id: [1; 32],
            parents: vec![[0; 32]],
            hlc: 12346,
            payload: vec![1, 2, 3],
        };

        buffer.push(delta.clone()).unwrap();
        assert_eq!(buffer.len(), 1);

        let drained = buffer.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].id, [1; 32]);
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_delta_buffer_overflow() {
        let mut buffer = DeltaBuffer::new(2, 0);

        buffer
            .push(BufferedDelta {
                id: [1; 32],
                parents: vec![],
                hlc: 1,
                payload: vec![],
            })
            .unwrap();

        buffer
            .push(BufferedDelta {
                id: [2; 32],
                parents: vec![],
                hlc: 2,
                payload: vec![],
            })
            .unwrap();

        let result = buffer.push(BufferedDelta {
            id: [3; 32],
            parents: vec![],
            hlc: 3,
            payload: vec![],
        });

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.buffered_count, 2);
    }

    #[test]
    fn test_sync_handshake_serialization() {
        let handshake = SyncHandshake {
            capabilities: SyncCapabilities::full(),
            root_hash: Hash::from([42; 32]),
            dag_heads: vec![[1; 32], [2; 32]],
            entity_count: 1000,
        };

        let encoded = borsh::to_vec(&handshake).unwrap();
        let decoded: SyncHandshake = borsh::from_slice(&encoded).unwrap();

        assert_eq!(decoded.root_hash, handshake.root_hash);
        assert_eq!(decoded.dag_heads, handshake.dag_heads);
        assert_eq!(decoded.entity_count, handshake.entity_count);
        assert!(decoded.capabilities.supports_compression);
    }

    // =========================================================================
    // Bloom Filter Tests
    // =========================================================================

    #[test]
    fn test_bloom_filter_insert_and_contains() {
        let mut filter = DeltaIdBloomFilter::with_capacity(100, 0.01);
        let id1 = [1u8; 32];
        let id2 = [2u8; 32];
        let id3 = [3u8; 32];

        // Initially empty
        assert!(filter.is_empty());
        assert!(!filter.maybe_contains(&id1));

        // Insert and check
        filter.insert(&id1);
        assert!(!filter.is_empty());
        assert_eq!(filter.len(), 1);
        assert!(filter.maybe_contains(&id1));
        assert!(!filter.maybe_contains(&id2)); // Definitely not present

        // Insert another
        filter.insert(&id2);
        assert_eq!(filter.len(), 2);
        assert!(filter.maybe_contains(&id2));
        assert!(!filter.maybe_contains(&id3)); // Definitely not present
    }

    #[test]
    fn test_bloom_filter_no_false_negatives() {
        let mut filter = DeltaIdBloomFilter::with_capacity(1000, 0.01);

        // Insert 100 random-ish IDs
        let ids: Vec<[u8; 32]> = (0..100u8)
            .map(|i| {
                let mut id = [0u8; 32];
                id[0] = i;
                id[31] = 255 - i;
                id
            })
            .collect();

        for id in &ids {
            filter.insert(id);
        }

        // All inserted IDs MUST be found (no false negatives)
        for id in &ids {
            assert!(filter.maybe_contains(id), "False negative for {:?}", id[0]);
        }
    }

    #[test]
    fn test_bloom_filter_serialization() {
        let mut filter = DeltaIdBloomFilter::with_capacity(100, 0.01);
        filter.insert(&[1u8; 32]);
        filter.insert(&[2u8; 32]);

        let encoded = borsh::to_vec(&filter).unwrap();
        let decoded: DeltaIdBloomFilter = borsh::from_slice(&encoded).unwrap();

        assert_eq!(decoded.len(), 2);
        assert!(decoded.maybe_contains(&[1u8; 32]));
        assert!(decoded.maybe_contains(&[2u8; 32]));
        assert!(!decoded.maybe_contains(&[3u8; 32]));
    }

    #[test]
    fn test_bloom_filter_filter_missing() {
        let mut filter = DeltaIdBloomFilter::with_capacity(100, 0.01);
        filter.insert(&[1u8; 32]);
        filter.insert(&[2u8; 32]);

        let query = [[1u8; 32], [2u8; 32], [3u8; 32], [4u8; 32]];
        let missing = filter.filter_missing(&query);

        // [3] and [4] are definitely missing
        assert!(missing.contains(&[3u8; 32]));
        assert!(missing.contains(&[4u8; 32]));
        // [1] and [2] should NOT be in missing (they're in the filter)
        assert!(!missing.contains(&[1u8; 32]));
        assert!(!missing.contains(&[2u8; 32]));
    }

    #[test]
    fn test_bloom_filter_size_and_fp_rate() {
        let filter = DeltaIdBloomFilter::with_capacity(1000, 0.01);

        // Should be reasonably sized (1% FP for 1000 items ≈ 1.2KB)
        assert!(filter.size_bytes() > 100);
        assert!(filter.size_bytes() < 10000);

        // Initial FP rate should be 0 (empty)
        assert_eq!(filter.estimated_fp_rate(), 0.0);
    }

    #[test]
    fn test_bloom_filter_default_for_sync() {
        let filter = DeltaIdBloomFilter::default_for_sync();

        // Should be ready for typical sync scenarios
        assert!(filter.is_empty());
        assert!(filter.size_bytes() > 0);
    }

    // =========================================================================
    // Gossip Mode Tests
    // =========================================================================

    #[test]
    fn test_gossip_mode_with_hints_always_includes() {
        let mode = GossipMode::WithHints;

        assert!(mode.should_include_hints(0));
        assert!(mode.should_include_hints(1));
        assert!(mode.should_include_hints(100));
        assert!(mode.should_include_hints(-50));
    }

    #[test]
    fn test_gossip_mode_minimal_never_includes() {
        let mode = GossipMode::Minimal;

        assert!(!mode.should_include_hints(0));
        assert!(!mode.should_include_hints(100));
        assert!(!mode.should_include_hints(-1000));
    }

    #[test]
    fn test_gossip_mode_adaptive_threshold() {
        let mode = GossipMode::Adaptive {
            entity_change_threshold: 10,
        };

        // Below threshold - no hints
        assert!(!mode.should_include_hints(0));
        assert!(!mode.should_include_hints(5));
        assert!(!mode.should_include_hints(-9));

        // At or above threshold - include hints
        assert!(mode.should_include_hints(10));
        assert!(mode.should_include_hints(-10));
        assert!(mode.should_include_hints(100));
    }

    #[test]
    fn test_gossip_mode_create_hints_with_hints() {
        let mode = GossipMode::WithHints;
        let root_hash = Hash::from([1u8; 32]);

        let hints = mode.create_hints(root_hash, 1000, 10, 5);
        assert!(hints.is_some());

        let hints = hints.unwrap();
        assert_eq!(hints.post_root_hash, root_hash);
        assert_eq!(hints.entity_count, 1000);
        assert_eq!(hints.tree_depth, 10);
    }

    #[test]
    fn test_gossip_mode_create_hints_minimal() {
        let mode = GossipMode::Minimal;
        let root_hash = Hash::from([2u8; 32]);

        // Minimal mode still returns hints but with zeroed metadata
        let hints = mode.create_hints(root_hash, 1000, 10, 5);
        assert!(hints.is_some());

        let hints = hints.unwrap();
        assert_eq!(hints.post_root_hash, root_hash); // Hash is always included
        assert_eq!(hints.entity_count, 0); // But metadata is zeroed
        assert_eq!(hints.tree_depth, 0);
    }

    #[test]
    fn test_gossip_mode_adaptive_creates_hints_when_threshold_met() {
        let mode = GossipMode::adaptive();
        let root_hash = Hash::from([3u8; 32]);

        // Large change - full hints
        let hints = mode.create_hints(root_hash, 1000, 10, 50);
        assert!(hints.is_some());
        let hints = hints.unwrap();
        assert_eq!(hints.entity_count, 1000);

        // Small change - minimal hints
        let hints = mode.create_hints(root_hash, 1000, 10, 5);
        assert!(hints.is_some());
        let hints = hints.unwrap();
        assert_eq!(hints.entity_count, 0); // Zeroed for small changes
    }

    #[test]
    fn test_gossip_mode_serialization() {
        let modes = [
            GossipMode::WithHints,
            GossipMode::Minimal,
            GossipMode::Adaptive {
                entity_change_threshold: 25,
            },
        ];

        for mode in modes {
            let encoded = borsh::to_vec(&mode).unwrap();
            let decoded: GossipMode = borsh::from_slice(&encoded).unwrap();
            assert_eq!(decoded, mode);
        }
    }

    #[test]
    fn test_gossip_mode_default_is_with_hints() {
        assert_eq!(GossipMode::default(), GossipMode::WithHints);
    }
}
