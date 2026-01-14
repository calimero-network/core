/// How often each node produces a hash heartbeat within the context (in seconds).
pub const HASH_HEARTBEAT_FREQUENCY_S: u64 = 30;

/// The period of cleanup of stale pending deltas (every 60 seconds) for the node (in seconds).
pub const PENDING_DELTAS_CLEANUP_FREQUENCY_S: u64 = 60;
/// The maximum age of the pending delta (in seconds).
pub const PENDING_DELTA_MAX_AGE_S: u64 = 300;

/// How many pending deltas are allowed before the snapshot fallback is triggered.
pub const PENDING_DELTA_SNAPSHOT_THRESHOLD: usize = 100;

/// The maximum age of the blob (in seconds).
pub const MAX_BLOB_AGE_S: u64 = 300;
/// The maximum number of cached blobs.
pub const MAX_BLOB_CACHE_COUNT: usize = 100;
/// The maximum size of the blob cache (in bytes).
/// Currently, equals to 500 MiB.
pub const MAX_BLOB_CACHE_SIZE_BYTES: usize = 500 * 1024 * 1024;
/// The period of eviction of old blobs (every 300 seconds) for the node (in seconds).
pub const OLD_BLOBS_EVICTION_FREQUENCY_S: u64 = 60;
