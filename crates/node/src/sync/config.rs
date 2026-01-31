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

/// Default maximum wait time for gossipsub mesh to form (20 seconds)
/// After a node restarts or joins a context, gossipsub needs time to exchange
/// GRAFT messages and form the mesh. This is the maximum time we'll wait.
pub const DEFAULT_MESH_FORMATION_TIMEOUT_SECS: u64 = 20;

/// Default interval between mesh formation checks (1 second)
pub const DEFAULT_MESH_FORMATION_CHECK_INTERVAL_MS: u64 = 1000;

/// Default snapshot chunk size for full resync (64 KB)
pub const DEFAULT_SNAPSHOT_CHUNK_SIZE: usize = 64 * 1024;

/// Default delta sync threshold (switch to full resync after this many deltas)
pub const DEFAULT_DELTA_SYNC_THRESHOLD: usize = 128;

/// Default threshold for adaptive strategy: use snapshot if peer has more than this many deltas
pub const DEFAULT_ADAPTIVE_SNAPSHOT_THRESHOLD: usize = 10;

/// Default divergence threshold for adaptive state sync (50%)
pub const DEFAULT_SNAPSHOT_DIVERGENCE_THRESHOLD: f32 = 0.5;

/// Default entity count threshold for bloom filter sync
pub const DEFAULT_BLOOM_FILTER_THRESHOLD: usize = 50;

/// Default tree depth threshold for subtree prefetch
pub const DEFAULT_SUBTREE_PREFETCH_DEPTH: usize = 3;

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

/// Strategy for Merkle tree state synchronization.
///
/// Controls which protocol is used when comparing state between nodes.
/// Each protocol has different trade-offs for round trips, bandwidth, and complexity.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum StateSyncStrategy {
    /// Automatic protocol selection based on tree characteristics.
    ///
    /// Analyzes tree depth, entity count, and divergence to choose optimal protocol:
    /// - Fresh node / >50% divergence → Snapshot
    /// - Deep tree (>3 levels) with few differing subtrees → SubtreePrefetch
    /// - Large tree (>50 entities) with <10% divergence → BloomFilter
    /// - Wide shallow tree (≤2 levels, >5 children) → LevelWise
    /// - Default → HashComparison
    #[default]
    Adaptive,

    /// Standard recursive hash comparison.
    ///
    /// Compare root hash → if different, compare children → recurse.
    /// - Round trips: O(depth * differing_branches)
    /// - Best for: General purpose, moderate divergence
    HashComparison,

    /// Full state snapshot transfer.
    ///
    /// Transfer entire state in one request.
    /// - Round trips: 1
    /// - Best for: Fresh nodes, large divergence (>50%)
    Snapshot,

    /// Compressed snapshot transfer.
    ///
    /// Full state transfer with zstd compression.
    /// - Round trips: 1
    /// - Best for: Large state (>100 entities), bandwidth constrained
    CompressedSnapshot,

    /// Bloom filter quick diff detection.
    ///
    /// Send compact representation of local entity IDs, receive missing entities.
    /// - Round trips: 2 (send filter, receive diff)
    /// - Best for: Large tree (>50 entities), small divergence (<10%)
    BloomFilter {
        /// False positive rate (default: 1%)
        false_positive_rate: f32,
    },

    /// Subtree prefetch for deep trees.
    ///
    /// When subtree differs, fetch entire subtree in one request.
    /// - Round trips: 1 + differing_subtrees
    /// - Best for: Deep trees (>3 levels), localized changes
    SubtreePrefetch {
        /// Maximum depth to prefetch (None = entire subtree)
        max_depth: Option<usize>,
    },

    /// Level-wise breadth-first sync.
    ///
    /// Sync one tree level at a time, batching requests per depth.
    /// - Round trips: O(depth)
    /// - Best for: Wide shallow trees (≤2 levels, many children)
    LevelWise {
        /// Maximum depth to sync (None = full tree)
        max_depth: Option<usize>,
    },
}

impl StateSyncStrategy {
    /// Create bloom filter strategy with default false positive rate.
    #[must_use]
    pub fn bloom_filter() -> Self {
        Self::BloomFilter {
            false_positive_rate: 0.01, // 1%
        }
    }

    /// Create subtree prefetch strategy with no depth limit.
    #[must_use]
    pub fn subtree_prefetch() -> Self {
        Self::SubtreePrefetch { max_depth: None }
    }

    /// Create level-wise strategy with no depth limit.
    #[must_use]
    pub fn level_wise() -> Self {
        Self::LevelWise { max_depth: None }
    }

    /// Check if this is an adaptive strategy.
    #[must_use]
    pub fn is_adaptive(&self) -> bool {
        matches!(self, Self::Adaptive)
    }

    /// Choose the appropriate protocol based on tree characteristics.
    ///
    /// Only used when strategy is `Adaptive`.
    ///
    /// # Safety
    ///
    /// **CRITICAL**: Snapshot/CompressedSnapshot are ONLY used for fresh nodes
    /// (where `local_has_data == false`). For initialized nodes, we ALWAYS use
    /// merge-aware protocols (HashComparison, BloomFilter, etc.) to preserve
    /// local changes via CRDT merge semantics.
    #[must_use]
    pub fn choose_protocol(
        local_has_data: bool,
        local_entity_count: usize,
        remote_entity_count: usize,
        tree_depth: usize,
        child_count: usize,
    ) -> Self {
        // Fresh node: use snapshot (safe - no local data to lose)
        if !local_has_data {
            return if remote_entity_count > 100 {
                Self::CompressedSnapshot
            } else {
                Self::Snapshot
            };
        }

        // ========================================================
        // INITIALIZED NODE: NEVER use Snapshot - it would lose local changes!
        // All protocols below use CRDT merge to preserve both sides.
        // ========================================================

        // Calculate estimated divergence
        let count_diff =
            (remote_entity_count as isize - local_entity_count as isize).unsigned_abs();
        let divergence_ratio = count_diff as f32 / remote_entity_count.max(1) as f32;

        // Large divergence (>50%): use HashComparison with CRDT merge
        // NOTE: We do NOT use Snapshot here because it would overwrite local data!
        // HashComparison + CRDT merge preserves both local and remote changes.
        if divergence_ratio > DEFAULT_SNAPSHOT_DIVERGENCE_THRESHOLD && remote_entity_count > 20 {
            // For large divergence, HashComparison is slower but SAFE
            // It will merge each entity using CRDT semantics
            return Self::HashComparison;
        }

        // Deep tree with few differing subtrees: use subtree prefetch
        if tree_depth > DEFAULT_SUBTREE_PREFETCH_DEPTH && child_count < 10 {
            return Self::SubtreePrefetch { max_depth: None };
        }

        // Large tree with small diff: use Bloom filter
        if remote_entity_count > DEFAULT_BLOOM_FILTER_THRESHOLD && divergence_ratio < 0.1 {
            return Self::bloom_filter();
        }

        // Wide shallow tree: use level-wise
        if tree_depth <= 2 && child_count > 5 {
            return Self::level_wise();
        }

        // Default: standard hash comparison
        Self::HashComparison
    }
}

impl std::fmt::Display for StateSyncStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Adaptive => write!(f, "adaptive"),
            Self::HashComparison => write!(f, "hash"),
            Self::Snapshot => write!(f, "snapshot"),
            Self::CompressedSnapshot => write!(f, "compressed"),
            Self::BloomFilter {
                false_positive_rate,
            } => {
                write!(f, "bloom:{:.2}", false_positive_rate)
            }
            Self::SubtreePrefetch { max_depth } => match max_depth {
                Some(d) => write!(f, "subtree:{}", d),
                None => write!(f, "subtree"),
            },
            Self::LevelWise { max_depth } => match max_depth {
                Some(d) => write!(f, "level:{}", d),
                None => write!(f, "level"),
            },
        }
    }
}

impl FromStr for StateSyncStrategy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.to_lowercase();
        match s.as_str() {
            "adaptive" | "auto" => Ok(Self::Adaptive),
            "hash" | "hashcomparison" => Ok(Self::HashComparison),
            "snapshot" => Ok(Self::Snapshot),
            "compressed" | "compressedsnapshot" => Ok(Self::CompressedSnapshot),
            "bloom" | "bloomfilter" => Ok(Self::bloom_filter()),
            "subtree" | "subtreeprefetch" => Ok(Self::subtree_prefetch()),
            "level" | "levelwise" => Ok(Self::level_wise()),
            _ => {
                // Handle parameterized variants
                if let Some(rate_str) = s.strip_prefix("bloom:") {
                    let rate = rate_str
                        .parse()
                        .map_err(|_| format!("Invalid bloom filter rate in '{}'", s))?;
                    Ok(Self::BloomFilter {
                        false_positive_rate: rate,
                    })
                } else if let Some(depth_str) = s.strip_prefix("subtree:") {
                    let depth = depth_str
                        .parse()
                        .map_err(|_| format!("Invalid subtree depth in '{}'", s))?;
                    Ok(Self::SubtreePrefetch {
                        max_depth: Some(depth),
                    })
                } else if let Some(depth_str) = s.strip_prefix("level:") {
                    let depth = depth_str
                        .parse()
                        .map_err(|_| format!("Invalid level depth in '{}'", s))?;
                    Ok(Self::LevelWise {
                        max_depth: Some(depth),
                    })
                } else {
                    Err(format!(
                        "Unknown strategy '{}'. Valid: adaptive, hash, snapshot, compressed, \
                         bloom[:<rate>], subtree[:<depth>], level[:<depth>]",
                        s
                    ))
                }
            }
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

    /// Strategy for Merkle tree state synchronization.
    ///
    /// Controls which protocol is used when comparing state between nodes.
    /// Default: `Adaptive` for automatic protocol selection.
    pub state_sync_strategy: StateSyncStrategy,

    /// Maximum time to wait for gossipsub mesh to form.
    ///
    /// After a node restarts or joins a context, gossipsub needs time to
    /// exchange GRAFT messages with peers. This is the maximum wait time.
    pub mesh_formation_timeout: time::Duration,

    /// Interval between mesh formation checks.
    pub mesh_formation_check_interval: time::Duration,

    /// Force state sync even when DAG catchup would normally be used.
    ///
    /// **FOR BENCHMARKING ONLY**: When true, bypasses DAG catchup and forces
    /// the configured `state_sync_strategy` to be used even when DAG heads differ.
    ///
    /// This allows benchmarking bloom filter, hash comparison, subtree prefetch,
    /// and level-wise strategies in divergence scenarios where DAG history exists.
    ///
    /// Default: `false` (use DAG catchup when possible - optimal for production)
    pub force_state_sync: bool,

    /// Strategy for finding viable sync peers.
    ///
    /// Controls how candidates are selected for reconciliation:
    /// - `Baseline` (A0): Current mesh-only approach
    /// - `MeshFirst` (A1): Only mesh peers, fail if empty
    /// - `RecentFirst` (A2): Try LRU cache first, then mesh
    /// - `AddressBookFirst` (A3): Try persisted peers first
    /// - `ParallelFind` (A4): Query all sources in parallel
    /// - `HealthFiltered` (A5): Exclude peers with recent failures
    ///
    /// Default: `Baseline` for production
    pub peer_find_strategy: super::peer_finder::PeerFindStrategy,
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
            state_sync_strategy: StateSyncStrategy::default(),
            mesh_formation_timeout: time::Duration::from_secs(DEFAULT_MESH_FORMATION_TIMEOUT_SECS),
            mesh_formation_check_interval: time::Duration::from_millis(
                DEFAULT_MESH_FORMATION_CHECK_INTERVAL_MS,
            ),
            force_state_sync: false,
            peer_find_strategy: super::peer_finder::PeerFindStrategy::default(),
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

    /// Create config with a specific state sync strategy.
    #[must_use]
    pub fn with_state_sync_strategy(mut self, strategy: StateSyncStrategy) -> Self {
        self.state_sync_strategy = strategy;
        self
    }

    /// Enable forcing state sync even when DAG catchup would normally be used.
    ///
    /// **FOR BENCHMARKING ONLY**: Bypasses DAG catchup to test state sync strategies.
    #[must_use]
    pub fn with_force_state_sync(mut self, force: bool) -> Self {
        self.force_state_sync = force;
        self
    }

    /// Set the peer finding strategy.
    ///
    /// Controls how viable sync peers are discovered and selected.
    #[must_use]
    pub fn with_peer_find_strategy(
        mut self,
        strategy: super::peer_finder::PeerFindStrategy,
    ) -> Self {
        self.peer_find_strategy = strategy;
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

    #[test]
    fn test_state_sync_strategy_from_str() {
        assert_eq!(
            "adaptive".parse::<StateSyncStrategy>().unwrap(),
            StateSyncStrategy::Adaptive
        );
        assert_eq!(
            "hash".parse::<StateSyncStrategy>().unwrap(),
            StateSyncStrategy::HashComparison
        );
        assert_eq!(
            "snapshot".parse::<StateSyncStrategy>().unwrap(),
            StateSyncStrategy::Snapshot
        );
        assert_eq!(
            "compressed".parse::<StateSyncStrategy>().unwrap(),
            StateSyncStrategy::CompressedSnapshot
        );
        assert_eq!(
            "bloom".parse::<StateSyncStrategy>().unwrap(),
            StateSyncStrategy::bloom_filter()
        );
        assert_eq!(
            "bloom:0.05".parse::<StateSyncStrategy>().unwrap(),
            StateSyncStrategy::BloomFilter {
                false_positive_rate: 0.05
            }
        );
        assert_eq!(
            "subtree".parse::<StateSyncStrategy>().unwrap(),
            StateSyncStrategy::subtree_prefetch()
        );
        assert_eq!(
            "subtree:5".parse::<StateSyncStrategy>().unwrap(),
            StateSyncStrategy::SubtreePrefetch { max_depth: Some(5) }
        );
        assert_eq!(
            "level".parse::<StateSyncStrategy>().unwrap(),
            StateSyncStrategy::level_wise()
        );
        assert_eq!(
            "level:3".parse::<StateSyncStrategy>().unwrap(),
            StateSyncStrategy::LevelWise { max_depth: Some(3) }
        );
    }

    #[test]
    fn test_state_sync_strategy_display() {
        assert_eq!(StateSyncStrategy::Adaptive.to_string(), "adaptive");
        assert_eq!(StateSyncStrategy::HashComparison.to_string(), "hash");
        assert_eq!(StateSyncStrategy::Snapshot.to_string(), "snapshot");
        assert_eq!(
            StateSyncStrategy::CompressedSnapshot.to_string(),
            "compressed"
        );
        assert_eq!(StateSyncStrategy::bloom_filter().to_string(), "bloom:0.01");
        assert_eq!(
            StateSyncStrategy::BloomFilter {
                false_positive_rate: 0.05
            }
            .to_string(),
            "bloom:0.05"
        );
        assert_eq!(StateSyncStrategy::subtree_prefetch().to_string(), "subtree");
        assert_eq!(
            StateSyncStrategy::SubtreePrefetch { max_depth: Some(5) }.to_string(),
            "subtree:5"
        );
        assert_eq!(StateSyncStrategy::level_wise().to_string(), "level");
        assert_eq!(
            StateSyncStrategy::LevelWise { max_depth: Some(3) }.to_string(),
            "level:3"
        );
    }

    #[test]
    fn test_state_sync_choose_protocol() {
        // Fresh node → snapshot
        assert_eq!(
            StateSyncStrategy::choose_protocol(false, 0, 50, 2, 5),
            StateSyncStrategy::Snapshot
        );

        // Fresh node with large state → compressed
        assert_eq!(
            StateSyncStrategy::choose_protocol(false, 0, 150, 2, 5),
            StateSyncStrategy::CompressedSnapshot
        );

        // Large divergence on INITIALIZED node → HashComparison (NOT snapshot!)
        // Snapshot would lose local data, so we use merge-aware protocol
        assert_eq!(
            StateSyncStrategy::choose_protocol(true, 10, 100, 2, 5),
            StateSyncStrategy::HashComparison
        );

        // Deep tree with few children → subtree prefetch
        assert_eq!(
            StateSyncStrategy::choose_protocol(true, 50, 60, 5, 3),
            StateSyncStrategy::SubtreePrefetch { max_depth: None }
        );

        // Large tree with small divergence → bloom filter
        assert_eq!(
            StateSyncStrategy::choose_protocol(true, 95, 100, 2, 5),
            StateSyncStrategy::bloom_filter()
        );

        // Wide shallow tree → level-wise
        // Use values that don't hit bloom filter (remote_count <= 50 or divergence >= 0.1)
        assert_eq!(
            StateSyncStrategy::choose_protocol(true, 30, 40, 2, 10),
            StateSyncStrategy::level_wise()
        );

        // Default case → hash comparison
        assert_eq!(
            StateSyncStrategy::choose_protocol(true, 10, 15, 3, 5),
            StateSyncStrategy::HashComparison
        );
    }
}
