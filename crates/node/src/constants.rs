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
/// Hard upper bound on a single inbound blob received over the sync stream.
/// A peer advertising size==0 (unknown) is still capped at this limit.
/// u64 to match the wire-protocol `size: u64` field; usize would truncate on 32-bit targets.
pub const MAX_BLOB_STREAM_SIZE_BYTES: u64 = 500 * 1024 * 1024;
/// The period of eviction of old blobs (every 300 seconds) for the node (in seconds).
pub const OLD_BLOBS_EVICTION_FREQUENCY_S: u64 = 300;

/// Rate limit duration for delta buffer overflow warnings (in seconds).
/// Prevents log spam when buffer is under sustained pressure.
pub const DELTA_BUFFER_DROP_WARNING_RATE_LIMIT_S: u64 = 5;

/// Capacity of the dedicated network-event channel between the network
/// manager and the NodeManager bridge. Sized to absorb burst traffic; the
/// bridge applies backpressure (and, past `NETWORK_EVENT_MAX_PENDING_RETRIES`,
/// drops with a metric) once full.
pub const NETWORK_EVENT_CHANNEL_SIZE: usize = 1000;
/// Upper bound on overflow events buffered in the network-event channel's
/// async retry path before a true drop — roughly one channel's worth.
pub const NETWORK_EVENT_MAX_PENDING_RETRIES: usize = 1000;

/// Broadcast capacity for server-facing node events (supports many concurrent
/// WebSocket clients).
pub const EVENT_BROADCAST_CHANNEL_SIZE: usize = 256;
/// Buffer for queued context sync requests (absorbs bursts of context
/// joins/syncs).
pub const CTX_SYNC_CHANNEL_SIZE: usize = 64;
/// Buffer for queued namespace governance sync requests.
pub const NS_SYNC_CHANNEL_SIZE: usize = 16;
/// Buffer for queued namespace join requests.
pub const NS_JOIN_CHANNEL_SIZE: usize = 16;
/// Buffer for queued open-subgroup join requests.
pub const OPEN_SUBGROUP_JOIN_CHANNEL_SIZE: usize = 16;
/// Buffer for the execute path's locally-applied-delta notifications to the
/// node's in-memory DeltaStore.
pub const LOCAL_DELTA_CHANNEL_SIZE: usize = 256;

/// How often the gossipsub mesh-peer-count snapshot is logged per node
/// (in seconds). The snapshot is CI-observable evidence of actual mesh
/// state — the libp2p-gossipsub `Updating mesh, new mesh: {}` log reports
/// heartbeat-additions, not current mesh size, so without this signal
/// "the mesh is empty" can't be told apart from "the mesh is full and
/// nothing was added this tick."
pub const MESH_STATS_LOG_FREQUENCY_S: u64 = 30;
