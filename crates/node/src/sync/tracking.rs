//! Sync state tracking and sequence validation.
//!
//! **Observability**: Tracks sync history for diagnostics and metrics.
//! **Robustness**: Implements exponential backoff on failures.

use eyre::bail;
use libp2p::PeerId;
use tokio::time::{self, Instant};

/// Sync protocol type for tracking which protocol was used (internal metrics).
///
/// This is a simplified enum for internal tracking and metrics, mapping from
/// the full [`calimero_node_primitives::sync::SyncProtocol`] enum.
///
/// Note: With DAG-based sync, we don't have active sync protocols.
/// State propagates automatically via gossipsub BroadcastMessage::StateDelta.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SyncProtocol {
    /// No active sync - DAG uses gossipsub broadcast
    None,
    /// DAG catchup via heads request (for newly joined nodes)
    DagCatchup,
    /// Full snapshot sync (used when delta sync is not possible)
    SnapshotSync,
}

impl From<&calimero_node_primitives::sync::SyncProtocol> for SyncProtocol {
    /// Maps the full 7-variant `SyncProtocol` from primitives to the internal 3-variant
    /// tracking enum for metrics purposes.
    fn from(p: &calimero_node_primitives::sync::SyncProtocol) -> Self {
        use calimero_node_primitives::sync::SyncProtocol as P;
        match p {
            P::None => Self::None,
            P::DeltaSync { .. } => Self::DagCatchup,
            P::Snapshot { .. }
            | P::HashComparison { .. }
            | P::BloomFilter { .. }
            | P::SubtreePrefetch { .. }
            | P::LevelWise { .. } => Self::SnapshotSync,
        }
    }
}

/// Tracks sync state and history for a context.
///
/// Maintains sync history, failure tracking, and implements exponential backoff
/// for contexts that repeatedly fail to sync.
#[derive(Debug, Clone)]
pub(crate) struct SyncState {
    /// Last sync time (None = sync in progress or never synced)
    last_sync: Option<Instant>,

    /// Last peer we successfully synced with
    last_peer: Option<PeerId>,

    /// Consecutive sync failures (resets on success)
    failure_count: u32,

    /// Last sync error message (for diagnostics)
    last_error: Option<String>,

    /// Total successful syncs (lifetime counter)
    pub success_count: u64,

    /// Last protocol used (Delta, Full, State)
    last_protocol: Option<SyncProtocol>,
}

impl SyncState {
    /// Create new sync state (never synced)
    pub(crate) fn new() -> Self {
        Self {
            last_sync: None,
            last_peer: None,
            failure_count: 0,
            last_error: None,
            success_count: 0,
            last_protocol: None,
        }
    }

    /// Mark sync as started (prevents concurrent syncs)
    pub(crate) fn start(&mut self) {
        self.last_sync = None; // In progress
    }

    /// Mark sync as successful
    pub(crate) fn on_success(&mut self, peer: PeerId, protocol: SyncProtocol) {
        self.last_sync = Some(Instant::now());
        self.last_peer = Some(peer);
        self.failure_count = 0;
        self.last_error = None;
        self.success_count += 1;
        self.last_protocol = Some(protocol);
    }

    /// Mark sync as failed
    pub(crate) fn on_failure(&mut self, error: String) {
        self.last_sync = Some(Instant::now()); // Not in progress anymore
        self.failure_count += 1;
        self.last_error = Some(error);
    }

    /// Calculate exponential backoff delay based on failure count
    pub(crate) fn backoff_delay(&self) -> time::Duration {
        // Exponential backoff: 2^failures seconds, max 5 minutes
        let backoff_secs = 2u64.pow(self.failure_count.min(8));
        time::Duration::from_secs(backoff_secs.min(300))
    }

    /// Get last sync time
    pub(crate) fn last_sync(&self) -> Option<Instant> {
        self.last_sync
    }

    /// Get failure count
    pub(crate) fn failure_count(&self) -> u32 {
        self.failure_count
    }

    /// Take last_sync value (for marking sync start while keeping old time)
    pub(crate) fn take_last_sync(&mut self) -> Option<Instant> {
        self.last_sync.take()
    }
}

impl Default for SyncState {
    fn default() -> Self {
        Self::new()
    }
}

/// Sequence ID generator and validator for protocol messages.
///
/// Ensures messages are processed in order during sync protocols.
/// Prevents message reordering attacks and protocol confusion.
#[derive(Debug, Default)]
pub(crate) struct Sequencer {
    current: usize,
}

impl Sequencer {
    /// Get next sequence ID and advance counter.
    pub(crate) fn next(&mut self) -> usize {
        let id = self.current;
        self.current += 1;
        id
    }

    /// Validate and advance to expected sequence ID.
    ///
    /// # Errors
    ///
    /// Returns error if the provided ID doesn't match the expected sequence.
    /// This indicates out-of-order messages or a protocol violation.
    pub(crate) fn expect(&mut self, expected: usize) -> eyre::Result<()> {
        if self.current != expected {
            bail!("sequence error: expected {}, at {}", expected, self.current);
        }

        self.current += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_node_primitives::sync::SyncProtocol as PrimitivesSyncProtocol;

    /// Test that SyncProtocol::None maps to tracking::SyncProtocol::None
    #[test]
    fn test_from_primitives_none() {
        let primitive = PrimitivesSyncProtocol::None;
        let tracking: SyncProtocol = (&primitive).into();
        assert!(matches!(tracking, SyncProtocol::None));
    }

    /// Test that DeltaSync maps to DagCatchup
    #[test]
    fn test_from_primitives_delta_sync() {
        let primitive = PrimitivesSyncProtocol::DeltaSync {
            missing_delta_ids: vec![[1; 32], [2; 32]],
        };
        let tracking: SyncProtocol = (&primitive).into();
        assert!(matches!(tracking, SyncProtocol::DagCatchup));
    }

    /// Test that Snapshot maps to SnapshotSync
    #[test]
    fn test_from_primitives_snapshot() {
        let primitive = PrimitivesSyncProtocol::Snapshot {
            compressed: true,
            verified: true,
        };
        let tracking: SyncProtocol = (&primitive).into();
        assert!(matches!(tracking, SyncProtocol::SnapshotSync));
    }

    /// Test that HashComparison maps to SnapshotSync (fallback category)
    #[test]
    fn test_from_primitives_hash_comparison() {
        let primitive = PrimitivesSyncProtocol::HashComparison {
            root_hash: [3; 32],
            divergent_subtrees: vec![],
        };
        let tracking: SyncProtocol = (&primitive).into();
        assert!(matches!(tracking, SyncProtocol::SnapshotSync));
    }

    /// Test that BloomFilter maps to SnapshotSync (fallback category)
    #[test]
    fn test_from_primitives_bloom_filter() {
        let primitive = PrimitivesSyncProtocol::BloomFilter {
            filter_size: 1000,
            false_positive_rate: 0.01,
        };
        let tracking: SyncProtocol = (&primitive).into();
        assert!(matches!(tracking, SyncProtocol::SnapshotSync));
    }

    /// Test that SubtreePrefetch maps to SnapshotSync (fallback category)
    #[test]
    fn test_from_primitives_subtree_prefetch() {
        let primitive = PrimitivesSyncProtocol::SubtreePrefetch {
            subtree_roots: vec![[4; 32]],
        };
        let tracking: SyncProtocol = (&primitive).into();
        assert!(matches!(tracking, SyncProtocol::SnapshotSync));
    }

    /// Test that LevelWise maps to SnapshotSync (fallback category)
    #[test]
    fn test_from_primitives_levelwise() {
        let primitive = PrimitivesSyncProtocol::LevelWise { max_depth: 2 };
        let tracking: SyncProtocol = (&primitive).into();
        assert!(matches!(tracking, SyncProtocol::SnapshotSync));
    }

    /// Test that all 7 primitives variants are covered (exhaustiveness check)
    #[test]
    fn test_from_primitives_all_variants_covered() {
        // This test ensures we handle all variants - if a new variant is added
        // to PrimitivesSyncProtocol, this test will fail to compile
        let variants: Vec<PrimitivesSyncProtocol> = vec![
            PrimitivesSyncProtocol::None,
            PrimitivesSyncProtocol::DeltaSync {
                missing_delta_ids: vec![],
            },
            PrimitivesSyncProtocol::HashComparison {
                root_hash: [0; 32],
                divergent_subtrees: vec![],
            },
            PrimitivesSyncProtocol::Snapshot {
                compressed: false,
                verified: false,
            },
            PrimitivesSyncProtocol::BloomFilter {
                filter_size: 0,
                false_positive_rate: 0.0,
            },
            PrimitivesSyncProtocol::SubtreePrefetch {
                subtree_roots: vec![],
            },
            PrimitivesSyncProtocol::LevelWise { max_depth: 0 },
        ];

        // All variants should convert without panic
        for variant in &variants {
            let _tracking: SyncProtocol = variant.into();
        }

        assert_eq!(variants.len(), 7, "Expected 7 SyncProtocol variants");
    }
}
