//! Synchronization configuration with sensible defaults.
//!
//! **Convention over Configuration**: All magic numbers are extracted to named constants.
//!
//! ## Design Philosophy
//!
//! Our DAG-based CRDT system uses a **dual-path approach** for delta propagation:
//!
//! 1. **Primary Path: Gossipsub Broadcast** (instant, <1s)
//!    - Deltas broadcast immediately when transactions complete
//!    - Fast, reliable in good network conditions
//!    - May fail due to network partitions, packet loss, etc.
//!
//! 2. **Fallback Path: Periodic Sync** (configurable, default 10s)
//!    - Nodes periodically exchange DAG heads and fetch missing deltas
//!    - Ensures eventual consistency even if broadcasts fail
//!    - MUST be aggressive enough to prevent divergence
//!
//! **Critical**: If periodic sync is too slow (e.g., 60s), nodes can diverge for extended
//! periods when broadcasts fail. The defaults below balance network overhead with convergence speed.
//!
//! ## Fresh Node Strategies
//!
//! When a new node joins with empty state, different sync strategies have tradeoffs:
//!
//! | Strategy | Speed | Network | Use Case |
//! |----------|-------|---------|----------|
//! | Snapshot | Fast (1 request) | High bandwidth | Production, large state |
//! | DeltaSync | Slow (N requests) | Low bandwidth | Testing DAG, small state |
//! | Adaptive | Variable | Balanced | General purpose |

use std::str::FromStr;

use tokio::time;

/// Default timeout for entire sync operation (30 seconds)
pub const DEFAULT_SYNC_TIMEOUT_SECS: u64 = 30;

/// Default minimum interval between syncs for same context (5 seconds)
/// This allows rapid re-sync if broadcasts fail, ensuring fast CRDT convergence
pub const DEFAULT_SYNC_INTERVAL_SECS: u64 = 5;

/// Default frequency of periodic sync checks (10 seconds)
/// Aggressive fallback for when gossipsub broadcasts fail or are delayed
pub const DEFAULT_SYNC_FREQUENCY_SECS: u64 = 10;

/// Default maximum concurrent sync operations
pub const DEFAULT_MAX_CONCURRENT_SYNCS: usize = 30;

/// Default snapshot chunk size for full resync (64 KB)
pub const DEFAULT_SNAPSHOT_CHUNK_SIZE: usize = 64 * 1024;

/// Default delta sync threshold (switch to full resync after this many deltas)
pub const DEFAULT_DELTA_SYNC_THRESHOLD: usize = 128;

/// Default threshold for adaptive strategy: use snapshot if peer has more than this many deltas
pub const DEFAULT_ADAPTIVE_SNAPSHOT_THRESHOLD: usize = 10;

/// Strategy for syncing fresh (uninitialized) nodes.
///
/// This controls how a node with empty state bootstraps from peers.
/// Configurable for benchmarking and testing different approaches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FreshNodeStrategy {
    /// Always use snapshot sync for fresh nodes.
    ///
    /// **Fastest**: Single request transfers entire state.
    /// - Pro: Minimal round trips, fast bootstrap
    /// - Con: Higher bandwidth for single transfer
    /// - Best for: Production, large state, fast bootstrap needed
    #[default]
    Snapshot,

    /// Always use delta-by-delta sync for fresh nodes.
    ///
    /// **Slowest**: Fetches each delta individually from genesis.
    /// - Pro: Tests full DAG sync path, lower peak bandwidth
    /// - Con: O(n) round trips, slow for large history
    /// - Best for: Testing, debugging DAG sync, small state
    DeltaSync,

    /// Choose strategy based on peer's state size.
    ///
    /// **Balanced**: Uses snapshot if peer has many deltas, delta sync otherwise.
    /// - Pro: Optimal for varying state sizes
    /// - Con: Requires extra query to determine strategy
    /// - Best for: General purpose, mixed workloads
    Adaptive {
        /// Use snapshot if peer has more than this many DAG heads/deltas
        snapshot_threshold: usize,
    },
}

impl FreshNodeStrategy {
    /// Create adaptive strategy with default threshold.
    #[must_use]
    pub fn adaptive() -> Self {
        Self::Adaptive {
            snapshot_threshold: DEFAULT_ADAPTIVE_SNAPSHOT_THRESHOLD,
        }
    }

    /// Create adaptive strategy with custom threshold.
    #[must_use]
    pub fn adaptive_with_threshold(threshold: usize) -> Self {
        Self::Adaptive {
            snapshot_threshold: threshold,
        }
    }

    /// Determine if snapshot should be used based on peer's state.
    ///
    /// Returns `true` if snapshot sync should be used, `false` for delta sync.
    #[must_use]
    pub fn should_use_snapshot(&self, peer_dag_heads_count: usize) -> bool {
        match self {
            Self::Snapshot => true,
            Self::DeltaSync => false,
            Self::Adaptive { snapshot_threshold } => peer_dag_heads_count >= *snapshot_threshold,
        }
    }
}

impl std::fmt::Display for FreshNodeStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Snapshot => write!(f, "snapshot"),
            Self::DeltaSync => write!(f, "delta"),
            Self::Adaptive { snapshot_threshold } => {
                write!(f, "adaptive:{}", snapshot_threshold)
            }
        }
    }
}

impl FromStr for FreshNodeStrategy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.to_lowercase();
        if s == "snapshot" {
            Ok(Self::Snapshot)
        } else if s == "delta" || s == "deltasync" {
            Ok(Self::DeltaSync)
        } else if s == "adaptive" {
            Ok(Self::adaptive())
        } else if let Some(threshold_str) = s.strip_prefix("adaptive:") {
            let threshold = threshold_str
                .parse()
                .map_err(|_| format!("Invalid threshold in '{}'", s))?;
            Ok(Self::Adaptive {
                snapshot_threshold: threshold,
            })
        } else {
            Err(format!(
                "Unknown strategy '{}'. Valid: snapshot, delta, adaptive, adaptive:<threshold>",
                s
            ))
        }
    }
}

/// Synchronization configuration.
///
/// Controls timing, concurrency, and protocol behavior for node synchronization.
#[derive(Clone, Copy, Debug)]
pub struct SyncConfig {
    /// Timeout for entire sync operation
    pub timeout: time::Duration,

    /// Minimum interval between syncs for same context
    pub interval: time::Duration,

    /// Frequency of periodic sync checks
    pub frequency: time::Duration,

    /// Maximum concurrent sync operations
    pub max_concurrent: usize,

    /// Snapshot chunk size for full resync (bytes)
    pub snapshot_chunk_size: usize,

    /// Maximum delta gap before falling back to full resync
    pub delta_sync_threshold: usize,

    /// Strategy for syncing fresh (uninitialized) nodes.
    ///
    /// This controls how a node with empty state bootstraps from peers.
    /// Default: `Snapshot` for fastest bootstrap.
    pub fresh_node_strategy: FreshNodeStrategy,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            timeout: time::Duration::from_secs(DEFAULT_SYNC_TIMEOUT_SECS),
            interval: time::Duration::from_secs(DEFAULT_SYNC_INTERVAL_SECS),
            frequency: time::Duration::from_secs(DEFAULT_SYNC_FREQUENCY_SECS),
            max_concurrent: DEFAULT_MAX_CONCURRENT_SYNCS,
            snapshot_chunk_size: DEFAULT_SNAPSHOT_CHUNK_SIZE,
            delta_sync_threshold: DEFAULT_DELTA_SYNC_THRESHOLD,
            fresh_node_strategy: FreshNodeStrategy::default(),
        }
    }
}

impl SyncConfig {
    /// Create config with a specific fresh node strategy.
    #[must_use]
    pub fn with_fresh_node_strategy(mut self, strategy: FreshNodeStrategy) -> Self {
        self.fresh_node_strategy = strategy;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fresh_node_strategy_from_str() {
        assert_eq!(
            "snapshot".parse::<FreshNodeStrategy>().unwrap(),
            FreshNodeStrategy::Snapshot
        );
        assert_eq!(
            "delta".parse::<FreshNodeStrategy>().unwrap(),
            FreshNodeStrategy::DeltaSync
        );
        assert_eq!(
            "adaptive".parse::<FreshNodeStrategy>().unwrap(),
            FreshNodeStrategy::adaptive()
        );
        assert_eq!(
            "adaptive:50".parse::<FreshNodeStrategy>().unwrap(),
            FreshNodeStrategy::Adaptive {
                snapshot_threshold: 50
            }
        );
    }

    #[test]
    fn test_fresh_node_strategy_display() {
        assert_eq!(FreshNodeStrategy::Snapshot.to_string(), "snapshot");
        assert_eq!(FreshNodeStrategy::DeltaSync.to_string(), "delta");
        assert_eq!(
            FreshNodeStrategy::Adaptive {
                snapshot_threshold: 10
            }
            .to_string(),
            "adaptive:10"
        );
    }

    #[test]
    fn test_should_use_snapshot() {
        // Snapshot always returns true
        assert!(FreshNodeStrategy::Snapshot.should_use_snapshot(0));
        assert!(FreshNodeStrategy::Snapshot.should_use_snapshot(100));

        // DeltaSync always returns false
        assert!(!FreshNodeStrategy::DeltaSync.should_use_snapshot(0));
        assert!(!FreshNodeStrategy::DeltaSync.should_use_snapshot(100));

        // Adaptive depends on threshold
        let adaptive = FreshNodeStrategy::Adaptive {
            snapshot_threshold: 10,
        };
        assert!(!adaptive.should_use_snapshot(5));
        assert!(adaptive.should_use_snapshot(10));
        assert!(adaptive.should_use_snapshot(50));
    }
}
