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

use tokio::time;

// Re-export from primitives to maintain single source of truth
pub use calimero_node_primitives::sync::DEFAULT_DELTA_SYNC_THRESHOLD;

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

/// Default mesh discovery retries for initialized nodes.
/// Initialized nodes already have state and can afford to fail fast.
pub const DEFAULT_MESH_RETRIES_INITIALIZED: u32 = 3;

/// Default mesh discovery retry delay for initialized nodes (milliseconds).
pub const DEFAULT_MESH_RETRY_DELAY_MS_INITIALIZED: u64 = 500;

/// Default mesh discovery retries for uninitialized nodes.
/// Gossipsub mesh takes 5-10 heartbeats (~5-10s) to add a new subscriber.
/// Uninitialized nodes need a longer window to avoid getting stuck before
/// their first snapshot sync.
pub const DEFAULT_MESH_RETRIES_UNINITIALIZED: u32 = 10;

/// Default mesh discovery retry delay for uninitialized nodes (milliseconds).
pub const DEFAULT_MESH_RETRY_DELAY_MS_UNINITIALIZED: u64 = 1_000;

/// Max concurrent peer probes when looking for a peer with state.
/// Typical meshes are 2-20 peers; a pool of 4 is enough parallelism
/// that the tail is bounded by the fastest responder, without racing
/// the whole mesh simultaneously on larger deployments. The probe
/// itself is read-only (a single `DagHeadsRequest`), so parallelising
/// it does not risk racing on per-context sync state.
pub const DEFAULT_PEER_STATE_PROBE_CONCURRENCY: usize = 4;

/// Maximum number of *additional* mesh peers to try for missing-parent
/// fetches after the initial sync peer returns without fully resolving
/// the DAG. The initial peer attempt is not counted toward this budget.
///
/// Applies to both data-delta parent pulls (cold-start join_context, #2198)
/// and governance-op parent pulls (subgroup MemberAdded propagation, #2209).
pub const DEFAULT_PARENT_PULL_ADDITIONAL_PEERS: usize = 3;

/// Total wall-clock budget (milliseconds) for the cross-peer
/// missing-parent fetch loop, including the initial peer attempt.
/// When exhausted, the sync session returns an error rather than
/// reporting silent success on a partially-applied DAG.
pub const DEFAULT_PARENT_PULL_BUDGET_MS: u64 = 10_000;

/// Synchronization configuration.
///
/// Controls timing, concurrency, and protocol behavior for node synchronization.
#[derive(Copy, Clone, Debug)]
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

    /// Max concurrent peer probes in `find_peer_with_state`.
    pub peer_state_probe_concurrency: usize,

    /// Max additional mesh peers to try for missing-parent fetches
    /// after the initial sync peer returns without fully resolving
    /// the DAG. See [`DEFAULT_PARENT_PULL_ADDITIONAL_PEERS`].
    pub parent_pull_additional_peers: usize,

    /// Wall-clock budget for the cross-peer missing-parent fetch loop.
    /// See [`DEFAULT_PARENT_PULL_BUDGET_MS`].
    pub parent_pull_budget: time::Duration,
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
            peer_state_probe_concurrency: DEFAULT_PEER_STATE_PROBE_CONCURRENCY,
            parent_pull_additional_peers: DEFAULT_PARENT_PULL_ADDITIONAL_PEERS,
            parent_pull_budget: time::Duration::from_millis(DEFAULT_PARENT_PULL_BUDGET_MS),
        }
    }
}
