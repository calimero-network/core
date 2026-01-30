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
}
