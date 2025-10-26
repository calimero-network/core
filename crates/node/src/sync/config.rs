//! Synchronization configuration with sensible defaults.
//!
//! **Convention over Configuration**: All magic numbers are extracted to named constants.

use tokio::time;

/// Default timeout for entire sync operation (5 minutes)
pub const DEFAULT_SYNC_TIMEOUT_SECS: u64 = 300;

/// Default minimum interval between syncs for same context (30 seconds)
pub const DEFAULT_SYNC_INTERVAL_SECS: u64 = 30;

/// Default frequency of periodic sync checks (1 minute)
pub const DEFAULT_SYNC_FREQUENCY_SECS: u64 = 60;

/// Default maximum concurrent sync operations
pub const DEFAULT_MAX_CONCURRENT_SYNCS: usize = 30;

/// Default snapshot chunk size for full resync (64 KB)
pub const DEFAULT_SNAPSHOT_CHUNK_SIZE: usize = 64 * 1024;

/// Default delta sync threshold (switch to full resync after this many deltas)
pub const DEFAULT_DELTA_SYNC_THRESHOLD: usize = 128;

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
