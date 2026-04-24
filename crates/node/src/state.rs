use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_blobstore::BlobManager as BlobStore;
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
    pub(crate) blobstore: BlobStore,
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

/// Default staleness cutoff for handler gating. See `crate::handler_gating`
/// for the full rationale and threshold semantics. 5 s is "tolerates normal
/// network jitter and brief pending-cascade windows, skips multi-second
/// catch-up replay."
pub(crate) const DEFAULT_HANDLER_STALENESS_THRESHOLD: Duration = Duration::from_secs(5);

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
    /// Highest NTP64 HLC value observed per context, across every arrival
    /// path (pubsub, sync fetch, cascade, buffered replay). Used by the
    /// handler-gating predicate to detect stale deltas without a wall
    /// clock. See `crate::handler_gating` for semantics. Raw NTP64 value
    /// (upper 32 bits = seconds, lower 32 bits = fraction).
    pub(crate) max_seen_hlc: Arc<DashMap<ContextId, AtomicU64>>,
    /// Threshold for `is_behind`'s HLC-staleness arm. See module doc on
    /// `crate::handler_gating` for threshold semantics. `0` is a legal
    /// (strictest) value, not a sentinel for "disabled"; use
    /// `Duration::MAX` to disable gating.
    pub(crate) handler_staleness_threshold: Duration,
}

impl NodeState {
    pub(crate) fn blob_cache_handle(&self) -> Arc<DashMap<BlobId, CachedBlob>> {
        self.blob_cache.clone()
    }

    pub(crate) fn delta_stores_handle(&self) -> Arc<DashMap<ContextId, DeltaStore>> {
        self.delta_stores.clone()
    }

    pub(crate) fn pending_specialized_node_invites_handle(&self) -> PendingSpecializedNodeInvites {
        self.pending_specialized_node_invites.clone()
    }

    pub(crate) const fn accept_mock_tee(&self) -> bool {
        self.accept_mock_tee
    }

    pub(crate) const fn node_mode(&self) -> NodeMode {
        self.node_mode
    }

    pub(crate) fn new(accept_mock_tee: bool, node_mode: NodeMode) -> Self {
        Self::with_handler_staleness_threshold(
            accept_mock_tee,
            node_mode,
            DEFAULT_HANDLER_STALENESS_THRESHOLD,
        )
    }

    /// Construct a NodeState with an explicit handler-staleness threshold.
    /// Use this when tuning for strict ("only live pubsub receipts",
    /// threshold = 0) or loose ("effectively disabled", threshold =
    /// `Duration::MAX`) semantics. See `crate::handler_gating` for
    /// guidance.
    pub(crate) fn with_handler_staleness_threshold(
        accept_mock_tee: bool,
        node_mode: NodeMode,
        handler_staleness_threshold: Duration,
    ) -> Self {
        Self {
            blob_cache: Arc::new(DashMap::new()),
            delta_stores: Arc::new(DashMap::new()),
            pending_specialized_node_invites: new_pending_specialized_node_invites(),
            accept_mock_tee,
            node_mode,
            sync_sessions: Arc::new(DashMap::new()),
            max_seen_hlc: Arc::new(DashMap::new()),
            handler_staleness_threshold,
        }
    }

    /// Record that we have observed `hlc_ntp64` for `context_id`. Pushes
    /// `max_seen_hlc[context_id]` forward if the value is newer than the
    /// current max. Cheap, lock-free in the common case (CAS loop on a
    /// single `AtomicU64`). Call at every delta-arrival entry point; see
    /// `crate::handler_gating` for the full list.
    ///
    /// Takes a raw NTP64 `u64` so both `HybridTimestamp` callers (via
    /// `hlc.get_time().as_u64()`) and `BufferedDelta.hlc` callers (already
    /// `u64`) work without a conversion dance.
    pub(crate) fn observe_hlc(&self, context_id: &ContextId, hlc_ntp64: u64) {
        let raw = hlc_ntp64;
        // Fast path: entry exists — bump via CAS loop on the AtomicU64.
        if let Some(entry) = self.max_seen_hlc.get(context_id) {
            let atomic = entry.value();
            let mut current = atomic.load(Ordering::Relaxed);
            while raw > current {
                match atomic.compare_exchange_weak(
                    current,
                    raw,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return,
                    Err(observed) => current = observed,
                }
            }
            return;
        }
        // Slow path: first observation for this context. Insert-or-bump
        // in a single entry() call to handle races against another
        // observer creating the entry.
        self.max_seen_hlc
            .entry(*context_id)
            .and_modify(|atomic| {
                let mut current = atomic.load(Ordering::Relaxed);
                while raw > current {
                    match atomic.compare_exchange_weak(
                        current,
                        raw,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(observed) => current = observed,
                    }
                }
            })
            .or_insert_with(|| AtomicU64::new(raw));
    }

    /// Highest NTP64 HLC value observed for this context, or `None` if we
    /// have never seen a delta for it. Exposed primarily for tests and
    /// diagnostics; production call sites should use `is_behind`.
    pub(crate) fn max_seen_hlc(&self, context_id: &ContextId) -> Option<u64> {
        self.max_seen_hlc
            .get(context_id)
            .map(|entry| entry.value().load(Ordering::Relaxed))
    }

    /// Handler-gating predicate. Returns `true` when handlers should be
    /// *skipped* for a delta with the given HLC, per the two-arm rule in
    /// `crate::handler_gating`:
    ///
    /// 1. A sync session is active for `context_id` (explicit catch-up).
    /// 2. `max_seen_hlc(context_id) - delta.hlc` exceeds
    ///    `handler_staleness_threshold`.
    ///
    /// Call at every site that would dispatch application event handlers:
    /// direct-apply, cascaded-events execution, buffered-replay,
    /// restart-replay. When the predicate returns `true`, the caller is
    /// expected to clear the DB `events` blob via
    /// `DeltaStore::mark_events_executed` to prevent re-evaluation on
    /// restart.
    pub(crate) fn is_behind(&self, context_id: &ContextId, delta_hlc_ntp64: u64) -> bool {
        if self.should_buffer_delta(context_id) {
            return true;
        }
        let Some(max_raw) = self.max_seen_hlc(context_id) else {
            // No observation yet — can't be behind what we haven't seen.
            return false;
        };
        // NTP64 units: upper 32 bits = seconds, lower 32 bits = fraction.
        // Compute gap in milliseconds at full precision:
        // `gap_ms = (gap_ntp64 * 1000) >> 32`. Bounded: gap_ntp64 ≤ 2^64,
        // but in practice ≤ ~2^32 (years of wall-clock), so
        // `gap_ntp64 * 1000` cannot overflow u64.
        let gap_ntp64 = max_raw.saturating_sub(delta_hlc_ntp64);
        let gap_ms = gap_ntp64.saturating_mul(1000) >> 32;
        gap_ms > self.handler_staleness_threshold.as_millis() as u64
    }

    /// Check if we should buffer a delta (during snapshot sync).
    pub(crate) fn should_buffer_delta(&self, context_id: &ContextId) -> bool {
        self.sync_sessions
            .get(context_id)
            .is_some_and(|session| session.state.should_buffer_deltas())
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
                let should_warn = session.last_drop_warning.is_none_or(|last| {
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
                let _removed = self.blob_cache.remove(blob_id);
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

#[cfg(test)]
mod handler_gating_tests {
    use super::*;
    use crate::run::NodeMode;

    /// Build an NTP64 `u64` from a whole-seconds value. NTP64 lays seconds
    /// in the upper 32 bits; the lower 32 bits (fraction) stay zero.
    fn ntp64_from_secs(seconds: u64) -> u64 {
        seconds << 32
    }

    fn ctx(byte: u8) -> ContextId {
        ContextId::from([byte; 32])
    }

    #[test]
    fn no_observation_yet_is_not_behind() {
        let state = NodeState::new(false, NodeMode::Standard);
        // Never called observe_hlc → max is `None` → predicate must
        // return false regardless of delta HLC. Guards the "isolated
        // node" case in the doc: no peers observed, can't be behind.
        assert!(!state.is_behind(&ctx(0), ntp64_from_secs(100)));
    }

    #[test]
    fn delta_at_frontier_is_not_behind() {
        let state = NodeState::new(false, NodeMode::Standard);
        let c = ctx(1);
        let t = ntp64_from_secs(100);
        state.observe_hlc(&c, t);
        // Gap = 0 → not behind. This is the direct-apply path invariant:
        // observing immediately before the check guarantees `false`.
        assert!(!state.is_behind(&c, t));
    }

    #[test]
    fn stale_delta_beyond_threshold_is_behind() {
        let state = NodeState::new(false, NodeMode::Standard);
        let c = ctx(2);
        state.observe_hlc(&c, ntp64_from_secs(1_000));
        // Delta is 100 s behind the frontier; default threshold is 5 s.
        assert!(state.is_behind(&c, ntp64_from_secs(900)));
    }

    #[test]
    fn stale_delta_within_threshold_is_not_behind() {
        let state = NodeState::new(false, NodeMode::Standard);
        let c = ctx(3);
        state.observe_hlc(&c, ntp64_from_secs(1_000));
        // 3 s gap, 5 s threshold → tolerated.
        assert!(!state.is_behind(&c, ntp64_from_secs(997)));
    }

    #[test]
    fn zero_threshold_fires_only_at_ms_precision_frontier() {
        let state = NodeState::with_handler_staleness_threshold(
            false,
            NodeMode::Standard,
            Duration::from_millis(0),
        );
        let c = ctx(4);
        state.observe_hlc(&c, ntp64_from_secs(100));
        // At the frontier: not behind.
        assert!(!state.is_behind(&c, ntp64_from_secs(100)));
        // Sub-millisecond gap: rounds to 0 ms, `0 > 0` is false, not
        // behind. Threshold `0` is "any ms-scale gap skips", not "bit-exact
        // frontier" — the predicate operates at millisecond precision.
        let sub_ms_gap = ntp64_from_secs(100) - 1; // 1 NTP64 unit ≈ 233 ps
        assert!(!state.is_behind(&c, sub_ms_gap));
        // 2 ms behind: clearly skipped at threshold 0.
        let two_ms_gap = ntp64_from_secs(100) - ((2_u64 << 32) / 1000 + 1); // 2+ ms worth
        assert!(state.is_behind(&c, two_ms_gap));
    }

    #[test]
    fn max_threshold_only_gated_by_sync_session() {
        let state =
            NodeState::with_handler_staleness_threshold(false, NodeMode::Standard, Duration::MAX);
        let c = ctx(5);
        state.observe_hlc(&c, ntp64_from_secs(1_000_000));
        // Even a massive gap doesn't trigger the HLC arm with MAX threshold.
        assert!(!state.is_behind(&c, ntp64_from_secs(1)));
    }

    #[test]
    fn active_sync_session_always_flags_behind() {
        let state =
            NodeState::with_handler_staleness_threshold(false, NodeMode::Standard, Duration::MAX);
        let c = ctx(6);
        state.observe_hlc(&c, ntp64_from_secs(100));
        // Threshold arm disabled (MAX), but sync-session arm is the only
        // signal that gates: start a session and the predicate flips.
        state.start_sync_session(c, 0);
        assert!(state.is_behind(&c, ntp64_from_secs(100)));
    }

    #[test]
    fn observe_hlc_never_goes_backwards() {
        let state = NodeState::new(false, NodeMode::Standard);
        let c = ctx(7);
        state.observe_hlc(&c, ntp64_from_secs(1_000));
        state.observe_hlc(&c, ntp64_from_secs(500)); // older, should be ignored
        assert_eq!(state.max_seen_hlc(&c), Some(ntp64_from_secs(1_000)));
        state.observe_hlc(&c, ntp64_from_secs(2_000)); // newer, bumps
        assert_eq!(state.max_seen_hlc(&c), Some(ntp64_from_secs(2_000)));
    }

    #[test]
    fn per_context_isolation() {
        let state = NodeState::new(false, NodeMode::Standard);
        let a = ctx(0xAA);
        let b = ctx(0xBB);
        state.observe_hlc(&a, ntp64_from_secs(1_000));
        // Context B has no observations; its predicate is independent.
        assert!(!state.is_behind(&b, ntp64_from_secs(0)));
        assert_eq!(state.max_seen_hlc(&b), None);
    }
}
