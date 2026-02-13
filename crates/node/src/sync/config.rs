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
        }
    }
}
