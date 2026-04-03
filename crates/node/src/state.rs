use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_blobstore::BlobManager;
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::{blobs::BlobId, context::ContextId};
use dashmap::DashMap;
use tracing::{debug, warn};

use crate::constants;
use crate::delta_store::DeltaStore;
use crate::run::NodeMode;
use crate::specialized_node_invite_state::{
    new_pending_specialized_node_invites, PendingSpecializedNodeInvites,
};
use crate::sync::SyncManager;

/// Cached blob with access tracking for eviction
#[derive(Debug, Clone)]
pub struct CachedBlob {
    pub data: Arc<[u8]>,
    pub last_accessed: Instant,
}

impl CachedBlob {
    pub fn new(data: Arc<[u8]>) -> Self {
        Self {
            data,
            last_accessed: Instant::now(),
        }
    }

    pub fn touch(&mut self) {
        self.last_accessed = Instant::now();
    }
}

/// External service clients (injected dependencies)
#[derive(Debug, Clone)]
pub(crate) struct NodeClients {
    pub(crate) context: ContextClient,
    pub(crate) node: NodeClient,
}

/// Service managers (injected dependencies)
#[derive(Clone, Debug)]
pub(crate) struct NodeManagers {
    pub(crate) blobstore: BlobManager,
    pub(crate) sync: SyncManager,
}

/// State of a sync session for a context.
#[derive(Debug)]
pub(crate) enum SyncSessionState {
    /// Buffering deltas during snapshot sync.
    /// The sync_start_hlc is stored in the DeltaBuffer itself.
    BufferingDeltas,
}

impl SyncSessionState {
    /// Check if we should buffer incoming deltas.
    pub fn should_buffer_deltas(&self) -> bool {
        matches!(self, Self::BufferingDeltas)
    }
}

/// Active sync session for a context.
#[derive(Debug)]
pub(crate) struct SyncSession {
    /// Current state of the sync.
    pub state: SyncSessionState,
    /// Buffer for deltas received during sync.
    pub delta_buffer: calimero_node_primitives::delta_buffer::DeltaBuffer,
    /// Timestamp of last drop warning (for rate limiting).
    pub last_drop_warning: Option<Instant>,
}

/// Mutable runtime state
#[derive(Clone, Debug)]
pub(crate) struct NodeState {
    pub(crate) blob_cache: Arc<DashMap<BlobId, CachedBlob>>,
    pub(crate) delta_stores: Arc<DashMap<ContextId, DeltaStore>>,
    /// Pending specialized node invites (standard node side) - tracks context_id/inviter for incoming verifications
    pub(crate) pending_specialized_node_invites: PendingSpecializedNodeInvites,
    /// Whether to accept mock TEE attestation (from config, for testing only)
    pub(crate) accept_mock_tee: bool,
    /// Node operation mode (Standard or ReadOnly)
    pub(crate) node_mode: NodeMode,
    /// Active sync sessions (for delta buffering during snapshot sync).
    pub(crate) sync_sessions: Arc<DashMap<ContextId, SyncSession>>,
}

impl NodeState {
    pub(crate) fn new(accept_mock_tee: bool, node_mode: NodeMode) -> Self {
        Self {
            blob_cache: Arc::new(DashMap::new()),
            delta_stores: Arc::new(DashMap::new()),
            pending_specialized_node_invites: new_pending_specialized_node_invites(),
            accept_mock_tee,
            node_mode,
            sync_sessions: Arc::new(DashMap::new()),
        }
    }

    /// Check if we should buffer a delta (during snapshot sync).
    pub(crate) fn should_buffer_delta(&self, context_id: &ContextId) -> bool {
        self.sync_sessions
            .get(context_id)
            .map_or(false, |session| session.state.should_buffer_deltas())
    }

    /// Buffer a delta during snapshot sync (Invariant I6).
    ///
    /// Returns `Some(PushResult)` if there was an active session, `None` if no session.
    ///
    /// The `PushResult` indicates what happened:
    /// - `Added`: Delta was buffered successfully
    /// - `Duplicate`: Delta ID was already buffered (no action)
    /// - `Evicted(id)`: Delta was buffered but oldest was evicted
    /// - `DroppedZeroCapacity(id)`: Delta was dropped (zero capacity)
    ///
    /// If the buffer is full, the oldest delta is evicted (oldest-first policy)
    /// and a rate-limited warning is logged. Drops are tracked via metrics.
    pub(crate) fn buffer_delta(
        &self,
        context_id: &ContextId,
        delta: calimero_node_primitives::delta_buffer::BufferedDelta,
    ) -> Option<calimero_node_primitives::delta_buffer::PushResult> {
        use calimero_node_primitives::delta_buffer::PushResult;

        if let Some(mut session) = self.sync_sessions.get_mut(context_id) {
            let incoming_delta_id = delta.id;
            let result = session.delta_buffer.push(delta);

            if result.had_data_loss() {
                // A delta was lost - log rate-limited warning
                let should_warn = session.last_drop_warning.map_or(true, |last| {
                    last.elapsed()
                        > Duration::from_secs(constants::DELTA_BUFFER_DROP_WARNING_RATE_LIMIT_S)
                });

                if should_warn {
                    session.last_drop_warning = Some(Instant::now());
                    let (evicted_id, reason) = match &result {
                        PushResult::Evicted(id) => (id, "buffer overflow"),
                        PushResult::DroppedZeroCapacity(id) => (id, "zero capacity"),
                        _ => unreachable!(),
                    };
                    warn!(
                        %context_id,
                        lost_delta_id = ?evicted_id,
                        incoming_delta_id = ?incoming_delta_id,
                        reason = reason,
                        drops = session.delta_buffer.drops(),
                        buffer_size = session.delta_buffer.len(),
                        capacity = session.delta_buffer.capacity(),
                        "Delta buffer data loss - {} (I6 violation risk)",
                        reason
                    );
                }

                // TODO (#4): Export drops to Prometheus metrics
                // metrics::counter!("calimero_sync_buffer_drops", "context_id" => context_id.to_string()).increment(1);
            }

            Some(result)
        } else {
            None // No active session
        }
    }

    /// Start a sync session for a context (enables delta buffering).
    ///
    /// Buffer capacity defaults to 10,000 deltas per context.
    pub(crate) fn start_sync_session(&self, context_id: ContextId, sync_start_hlc: u64) {
        self.start_sync_session_with_capacity(
            context_id,
            sync_start_hlc,
            calimero_node_primitives::delta_buffer::DEFAULT_BUFFER_CAPACITY,
        );
    }

    /// Start a sync session with custom buffer capacity.
    ///
    /// # Capacity Warning (#7)
    ///
    /// If capacity is below `MIN_RECOMMENDED_CAPACITY`, a warning is logged.
    /// Zero capacity is valid but will drop ALL deltas.
    pub(crate) fn start_sync_session_with_capacity(
        &self,
        context_id: ContextId,
        sync_start_hlc: u64,
        capacity: usize,
    ) {
        use calimero_node_primitives::delta_buffer::{DeltaBuffer, MIN_RECOMMENDED_CAPACITY};

        // (#7) Warn if capacity is below recommended minimum
        if capacity < MIN_RECOMMENDED_CAPACITY {
            warn!(
                %context_id,
                capacity,
                min_recommended = MIN_RECOMMENDED_CAPACITY,
                "Delta buffer capacity below recommended minimum - may cause excessive data loss"
            );
        }

        debug!(
            %context_id,
            sync_start_hlc,
            capacity,
            "Starting sync session with delta buffering"
        );

        self.sync_sessions.insert(
            context_id,
            SyncSession {
                state: SyncSessionState::BufferingDeltas,
                delta_buffer: DeltaBuffer::new(capacity, sync_start_hlc),
                last_drop_warning: None,
            },
        );
    }

    /// End a sync session and return buffered deltas for replay.
    ///
    /// Call this after sync completes successfully. Buffered deltas should be
    /// replayed in FIFO order to preserve causality.
    pub(crate) fn end_sync_session(
        &self,
        context_id: &ContextId,
    ) -> Option<Vec<calimero_node_primitives::delta_buffer::BufferedDelta>> {
        if let Some((_, mut session)) = self.sync_sessions.remove(context_id) {
            let drops = session.delta_buffer.drops();
            let buffered_count = session.delta_buffer.len();

            if drops > 0 {
                warn!(
                    %context_id,
                    drops,
                    buffered_count,
                    "Sync session ended with {} dropped deltas (I6 partial violation)",
                    drops
                );
            } else {
                debug!(
                    %context_id,
                    buffered_count,
                    "Sync session ended successfully"
                );
            }

            Some(session.delta_buffer.drain())
        } else {
            None
        }
    }

    /// Cancel a sync session and discard buffered deltas.
    ///
    /// Call this on sync error/failure. Buffered deltas are discarded since
    /// the sync didn't complete and the context state may be inconsistent.
    pub(crate) fn cancel_sync_session(&self, context_id: &ContextId) {
        if let Some((_, session)) = self.sync_sessions.remove(context_id) {
            let drops = session.delta_buffer.drops();
            let buffered_count = session.delta_buffer.len();

            warn!(
                %context_id,
                buffered_count,
                drops,
                "Sync session cancelled - discarding buffered deltas"
            );
        }
    }

    /// Evict blobs from cache based on age, count, and memory limits
    pub(crate) fn evict_old_blobs(&self) {
        let now = Instant::now();
        let before_count = self.blob_cache.len();

        // Phase 1: Remove blobs older than MAX_BLOB_AGE
        self.blob_cache.retain(|_, cached_blob| {
            now.duration_since(cached_blob.last_accessed)
                < Duration::from_secs(constants::MAX_BLOB_AGE_S)
        });

        let after_time_eviction = self.blob_cache.len();

        // Phase 2: If still over count limit, remove least recently used
        if self.blob_cache.len() > constants::MAX_BLOB_CACHE_COUNT {
            let mut blobs: Vec<_> = self
                .blob_cache
                .iter()
                .map(|entry| (*entry.key(), entry.value().last_accessed))
                .collect();

            // Sort by last_accessed (oldest first)
            blobs.sort_by_key(|(_, accessed)| *accessed);

            // Remove oldest until under count limit
            let to_remove = self.blob_cache.len() - constants::MAX_BLOB_CACHE_COUNT;
            for (blob_id, _) in blobs.iter().take(to_remove) {
                let _removed = self.blob_cache.remove(&blob_id);
            }
        }

        let after_count_eviction = self.blob_cache.len();

        // Phase 3: If still over memory limit, remove by LRU until under budget
        let total_size: usize = self
            .blob_cache
            .iter()
            .map(|entry| entry.value().data.len())
            .sum();

        if total_size > constants::MAX_BLOB_CACHE_SIZE_BYTES {
            let mut blobs: Vec<_> = self
                .blob_cache
                .iter()
                .map(|entry| {
                    (
                        *entry.key(),
                        entry.value().last_accessed,
                        entry.value().data.len(),
                    )
                })
                .collect();

            // Sort by last_accessed (oldest first)
            blobs.sort_by_key(|(_, accessed, _)| *accessed);

            let mut current_size = total_size;
            let mut removed_count = 0;

            for (blob_id, _, size) in blobs {
                if current_size <= constants::MAX_BLOB_CACHE_SIZE_BYTES {
                    break;
                }
                let _removed = self.blob_cache.remove(&blob_id);
                current_size = current_size.saturating_sub(size);
                removed_count += 1;
            }

            if removed_count > 0 {
                #[expect(
                    clippy::integer_division,
                    reason = "MB conversion for logging, precision not critical"
                )]
                let freed_mb = total_size.saturating_sub(current_size) / 1024 / 1024;
                #[expect(
                    clippy::integer_division,
                    reason = "MB conversion for logging, precision not critical"
                )]
                let new_size_mb = current_size / 1024 / 1024;
                tracing::debug!(
                    removed_count,
                    freed_mb,
                    new_size_mb,
                    "Evicted blobs to stay under memory limit"
                );
            }
        }

        let total_evicted = before_count.saturating_sub(self.blob_cache.len());
        if total_evicted > 0 {
            tracing::debug!(
                total_evicted,
                time_evicted = before_count.saturating_sub(after_time_eviction),
                count_evicted = after_time_eviction.saturating_sub(after_count_eviction),
                memory_evicted = after_count_eviction.saturating_sub(self.blob_cache.len()),
                remaining_count = self.blob_cache.len(),
                "Blob cache eviction completed"
            );
        }
    }
}
