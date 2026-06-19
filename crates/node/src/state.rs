use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use calimero_blobstore::BlobManager as BlobStore;
use calimero_context_client::client::ContextClient;
use calimero_context_config::types::ContextGroupId;
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::SyncStatusSnapshot;
use calimero_primitives::identity::PublicKey;
use calimero_primitives::{
    blobs::BlobId,
    context::{ContextId, GroupMemberRole},
};
use dashmap::DashMap;
use libp2p::PeerId;
use tracing::{debug, warn};

use crate::constants;
use crate::delta_store::DeltaStore;
use crate::peer_identity_cache::{
    ObservedMembership, PeerIdentityCache, PeerScoreTier, PEER_IDENTITY_TTL_SECS,
};
use crate::run::NodeMode;
use crate::specialized_node_invite_state::{
    new_pending_specialized_node_invites, PendingSpecializedNodeInvites,
};
use crate::sync::SyncManager;

/// Current wall-clock unix seconds, used to stamp `last_seen` on cached
/// peer-identity observations. Wall-clock (not a monotonic `Instant`) so
/// freshness survives a process restart — the whole point of the cache.
/// A pre-epoch clock degrades to 0 (everything looks maximally old) rather
/// than panicking.
pub(crate) fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

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
    /// Shared unified-op projection registry (cutover-flip prerequisite). The
    /// same `Arc` the context manager feeds; the node reads it at the
    /// data-write decision for the authorize-vs-live shadow-compare. Default
    /// here; node startup replaces it with the instance shared with the manager.
    pub(crate) scope_projections:
        Arc<std::sync::RwLock<calimero_context::scope_projection::ScopeProjections>>,
    /// Active sync sessions (for delta buffering during snapshot sync).
    pub(crate) sync_sessions: Arc<DashMap<ContextId, SyncSession>>,
    /// Per-context queue of state deltas whose `governance_position` references
    /// governance heads that aren't yet known locally — i.e., the cross-DAG
    /// membership lookup returned `Unknown { needed }`, indicating the
    /// receiver's governance state hasn't caught up to what the sender
    /// signed against.
    ///
    /// Drained lazily on the next state-delta receive for the same context: each
    /// pending delta is re-evaluated via `membership_status_at`; if governance has
    /// caught up the delta is processed (applied or rejected by the cross-DAG
    /// check), otherwise it is pushed back. Lazy drain trades a small
    /// worst-case latency (until the
    /// next state delta arrives in the same context) for not having to plumb a
    /// notification path from the governance-apply path into this buffer.
    ///
    /// Per-context capacity is bounded by [`MAX_GOVERNANCE_PENDING_PER_CONTEXT`]
    /// with FIFO eviction of the oldest entry. Without the bound, a peer
    /// flooding deltas with unknown governance heads could exhaust memory; the
    /// hash-heartbeat divergence path will catch any legitimate eviction
    /// victim by triggering snapshot sync.
    pub(crate) governance_pending: Arc<
        DashMap<
            ContextId,
            std::collections::VecDeque<calimero_node_primitives::delta_buffer::BufferedDelta>,
        >,
    >,
    /// Cache of `peer_id → identities observed signing applied messages`,
    /// populated by `observe_peer_identity` after a state-delta or
    /// namespace-governance op from `peer_id` successfully applies
    /// (signature verified, nonce monotonic, cross-DAG membership check
    /// passed). Consumed by sync-peer selection to preferentially target
    /// peers in the trusted-anchor set (`{Owner} ∪ {Admins} ∪
    /// {ReadOnlyTee}` — see
    /// `calimero_context::group_store::trusted_anchors_for_group`).
    ///
    /// **Trust model**: entries reflect identities a peer has *proven*
    /// to control via signed-and-applied messages — spoofing is bounded
    /// by signature verification at apply time. The cache itself is not
    /// a trust gate: downstream reconcile paths verify received state
    /// against a signed expected hash, so cache poisoning that
    /// mis-routes a sync request gets caught at adoption time, never
    /// used to authorize anything. A `peer_id` legitimately maps to
    /// multiple identities — a node hosts one libp2p key but joins
    /// many contexts, each with its own signing identity.
    ///
    /// **Lifetime**: entries persist for the process lifetime. Peer
    /// disconnect does not evict — the same peer may reconnect and the
    /// mapping is still valid. Bounded by the unique
    /// `(peer_id, identity)` pairs observed, which is itself bounded by
    /// group member count.
    pub(crate) peer_identities: Arc<DashMap<PeerId, BTreeSet<PublicKey>>>,
    /// Durable backing for `peer_identities`: the same authenticated
    /// observations, structured per group with role + `last_seen`, so the
    /// membership signal survives a restart instead of being rebuilt from
    /// scratch by live traffic. `peer_identities` above stays the O(1) hot
    /// read path for anchor-preferred selection; this is snapshotted to a
    /// `Generic` datastore key on a tick and hydrated (into both itself and
    /// `peer_identities`) on startup. Written only from the authenticated
    /// `observe_peer_identity` gate, and never a trust gate — see that
    /// field's trust-model docs.
    pub(crate) peer_identity_cache: Arc<Mutex<PeerIdentityCache>>,
    /// Last gossipsub app-specific score tier pushed to the network layer
    /// per peer (#2513). The reconciler on the snapshot tick diffs the
    /// desired tiers (derived from `peer_identity_cache`) against this and
    /// pushes only the changes, so a peer's score is updated on a
    /// membership *transition* rather than on every observed op.
    pub(crate) peer_scores: Arc<Mutex<BTreeMap<PeerId, PeerScoreTier>>>,
    /// Per-context reconcile-after-divergence attempt state. Used by the
    /// sync manager to apply exponential backoff between successive
    /// reconcile attempts for the same context, so a persistently
    /// misbehaving anchor (or a transient bug that re-fires divergence
    /// every signed op) cannot wedge the node into a tight
    /// divergence → reconcile → divergence loop.
    ///
    /// Entry semantics:
    /// - Cleared on a successful post-adoption hash match.
    /// - Updated on every failure (sync error OR post-adoption
    ///   mismatch), with `consecutive_failures` incremented and
    ///   `last_attempt_at` set to now.
    /// - Cooldown computed as exponential backoff capped at 30 min;
    ///   see [`reconcile_cooldown`] in `sync::manager`.
    pub(crate) reconcile_attempts: Arc<DashMap<ContextId, ReconcileAttempt>>,
    /// Per-context, best-effort sync-progress snapshot published by the sync
    /// run-loop and read out-of-band (JSON-RPC `sync_status`). Lets a client
    /// blocked on `Uninitialized` tell "syncing" from "stuck" instead of
    /// guessing. Advisory only — see [`SyncStatusSnapshot`] — and absent for
    /// contexts the run-loop has never touched.
    pub(crate) sync_status: Arc<DashMap<ContextId, SyncStatusSnapshot>>,
}

/// Per-context backoff state for the reconcile-after-divergence path.
#[derive(Clone, Debug)]
pub(crate) struct ReconcileAttempt {
    pub(crate) last_attempt_at: Instant,
    pub(crate) consecutive_failures: u32,
}

/// Maximum number of state deltas that may sit in the governance-pending
/// buffer for a single context simultaneously. Exceeding this evicts the
/// oldest entry FIFO. Sized for normal partition-recovery — a few seconds
/// of held deltas at typical send rates — not for adversarial flooding.
pub(crate) const MAX_GOVERNANCE_PENDING_PER_CONTEXT: usize = 256;

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
        Self {
            blob_cache: Arc::new(DashMap::new()),
            delta_stores: Arc::new(DashMap::new()),
            pending_specialized_node_invites: new_pending_specialized_node_invites(),
            accept_mock_tee,
            node_mode,
            scope_projections: Arc::new(std::sync::RwLock::new(
                calimero_context::scope_projection::ScopeProjections::new(),
            )),
            sync_sessions: Arc::new(DashMap::new()),
            governance_pending: Arc::new(DashMap::new()),
            peer_identities: Arc::new(DashMap::new()),
            peer_identity_cache: Arc::new(Mutex::new(PeerIdentityCache::default())),
            peer_scores: Arc::new(Mutex::new(BTreeMap::new())),
            reconcile_attempts: Arc::new(DashMap::new()),
            sync_status: Arc::new(DashMap::new()),
        }
    }

    /// Shared handle to the sync-status map, for the run-loop publisher.
    pub(crate) fn sync_status_handle(&self) -> Arc<DashMap<ContextId, SyncStatusSnapshot>> {
        self.sync_status.clone()
    }

    /// Read the latest sync-status snapshot for a context, if the run-loop has
    /// recorded one. `None` means the context has had no sync activity (e.g.
    /// it was created locally, or just joined and not yet dispatched).
    pub(crate) fn sync_status_snapshot(
        &self,
        context_id: &ContextId,
    ) -> Option<SyncStatusSnapshot> {
        self.sync_status.get(context_id).map(|s| s.value().clone())
    }

    /// Record that `peer_id` has successfully delivered a message
    /// authored by `identity`. Called from receive paths after the
    /// message verifies and applies — see field-level docs on
    /// `peer_identities` for the trust model. Idempotent.
    ///
    /// `membership` carries the group + role the caller resolved at the
    /// authenticated cut, when it has them cheaply (the state-delta path
    /// does; the namespace-governance path passes `None`). When present,
    /// the observation is also written through to the durable
    /// `peer_identity_cache` so it survives a restart; when absent, only
    /// the in-memory reverse view is updated (no regression — that view is
    /// rebuilt from live traffic as before).
    pub(crate) fn observe_peer_identity(
        &self,
        peer_id: PeerId,
        identity: PublicKey,
        membership: Option<ObservedMembership>,
    ) {
        let _inserted = self
            .peer_identities
            .entry(peer_id)
            .or_default()
            .insert(identity);

        if let Some(ObservedMembership { group_id, role }) = membership {
            // Read the clock before taking the lock so the guard isn't
            // held across the `SystemTime::now()` syscall (keeps the
            // critical section to the in-memory `record`).
            let now = now_unix_secs();
            self.lock_peer_identity_cache()
                .record(group_id, identity, peer_id, role, now);
        }
    }

    /// Lock the durable peer-identity cache, recovering the guard even if
    /// a prior holder panicked. The cache is a best-effort routing hint,
    /// so a poisoned lock should degrade to "use what's there" rather than
    /// propagate a panic into sync selection. The guard is only ever held
    /// across synchronous work (no `.await` in scope).
    pub(crate) fn lock_peer_identity_cache(&self) -> MutexGuard<'_, PeerIdentityCache> {
        self.peer_identity_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Lock the per-peer score-tier tracker, recovering a poisoned guard
    /// (same rationale as `lock_peer_identity_cache`). Held only across
    /// synchronous work in the score reconciler.
    pub(crate) fn lock_peer_scores(&self) -> MutexGuard<'_, BTreeMap<PeerId, PeerScoreTier>> {
        self.peer_scores
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Read-lock the unified-op scope-projections. The decision-site shadow's
    /// reads (`member_at_cut` / `namespace_to_refresh` / `cut_diagnostics`) take
    /// this on every authorized delta, so an `RwLock` read lets them run
    /// concurrently instead of serializing against each other and the
    /// governance-apply writer — the contention that otherwise starves apply and
    /// stalls churn-heavy scenarios. Recovers a poisoned guard rather than
    /// skipping (a poisoned writer must not silently blind the divergence gate).
    /// Held only across synchronous work (no `.await` in scope).
    pub(crate) fn read_scope_projections(
        &self,
    ) -> std::sync::RwLockReadGuard<'_, calimero_context::scope_projection::ScopeProjections> {
        self.scope_projections
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Write-lock the unified-op scope-projections (for `apply_backfill`). Brief
    /// and infrequent relative to the reads; see [`Self::read_scope_projections`].
    pub(crate) fn write_scope_projections(
        &self,
    ) -> std::sync::RwLockWriteGuard<'_, calimero_context::scope_projection::ScopeProjections> {
        self.scope_projections
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Push a state delta into the governance-pending buffer. Called when
    /// `membership_status_at` returns `Unknown { needed }` — the referenced
    /// governance heads aren't yet known locally, so the delta cannot be
    /// authorized until governance catches up.
    ///
    /// Skips the push if a delta with the same `id` is already in the
    /// queue (gossipsub re-delivers are common; double-buffering would
    /// re-apply the same delta twice when the drain fires).
    ///
    /// FIFO-evicts the oldest entry if pushing would exceed
    /// [`MAX_GOVERNANCE_PENDING_PER_CONTEXT`]. Eviction emits a warn log so
    /// operators can spot DoS-shaped traffic.
    pub(crate) fn buffer_governance_pending(
        &self,
        context_id: ContextId,
        delta: calimero_node_primitives::delta_buffer::BufferedDelta,
    ) {
        let mut entry = self.governance_pending.entry(context_id).or_default();
        // Deduplicate by delta_id — gossipsub re-delivery shouldn't
        // amplify the buffer or cause repeated re-apply work on drain.
        if entry.iter().any(|existing| existing.id == delta.id) {
            debug!(
                %context_id,
                delta_id = ?delta.id,
                "governance-pending buffer: skipping duplicate"
            );
            return;
        }
        if entry.len() >= MAX_GOVERNANCE_PENDING_PER_CONTEXT {
            let evicted = entry.pop_front();
            warn!(
                %context_id,
                evicted_id = ?evicted.as_ref().map(|d| d.id),
                cap = MAX_GOVERNANCE_PENDING_PER_CONTEXT,
                "governance-pending buffer at capacity; evicting oldest"
            );
        }
        entry.push_back(delta);
    }

    /// Pop the front-most pending delta for `context_id`, leaving the rest
    /// of the queue in place. Returns `None` if the buffer is empty.
    ///
    /// Used by the drain loop in
    /// `state_delta::drain_governance_pending` instead of a bulk
    /// drain-all-then-process: if `apply_authorized_state_delta` panics
    /// or the actor task is killed mid-iteration, only the in-flight
    /// delta is lost — the rest stay in the buffer and get re-tried by
    /// the next drain pass. The bulk-drain version (commit history) was
    /// flagged by review for losing every still-unprocessed delta on
    /// panic.
    pub(crate) fn pop_governance_pending(
        &self,
        context_id: &ContextId,
    ) -> Option<calimero_node_primitives::delta_buffer::BufferedDelta> {
        let mut entry = self.governance_pending.get_mut(context_id)?;
        let popped = entry.pop_front();
        // Don't remove the now-empty VecDeque here. A previous version of
        // this code did `drop(entry); remove(context_id)`, which had a
        // race: a concurrent `buffer_governance_pending` could insert a
        // fresh delta between the lock-drop and the remove, and the
        // remove would silently lose that newly-inserted delta. Leaving
        // an empty VecDeque costs ~24 bytes per context that ever had
        // pending entries, which is bounded and trivial. If empty-entry
        // accumulation ever matters, a periodic GC pass that holds the
        // entry-write-lock and `remove_if(|q| q.is_empty())` is the
        // race-free way to clean up.
        popped
    }

    /// Returns the current length of the governance-pending buffer for a
    /// context. Used by the drain loop's iteration cap so we can't get
    /// stuck draining indefinitely if a delta keeps re-buffering itself
    /// (the per-delta `governance_drain_attempts` counter is the deeper
    /// guard, but this is a cheap pre-check).
    pub(crate) fn governance_pending_len(&self, context_id: &ContextId) -> usize {
        self.governance_pending
            .get(context_id)
            .map(|q| q.len())
            .unwrap_or(0)
    }

    /// List every `ContextId` that currently has at least one entry in the
    /// governance-pending buffer. Used by the namespace-governance apply
    /// path to trigger drains across all affected contexts when a
    /// governance op lands — without this, the lazy on-state-delta drain
    /// alone deadlocks if the only state delta in flight is one waiting
    /// for that very governance op.
    pub(crate) fn governance_pending_context_ids(&self) -> Vec<ContextId> {
        self.governance_pending
            .iter()
            .map(|entry| *entry.key())
            .collect()
    }

    /// Distinct `source_peer`s of the deltas currently buffered for
    /// `context_id`, in first-seen order. Non-consuming.
    ///
    /// Used by the #2625 governance backfill to target the peers that
    /// actually delivered the stuck deltas. Such a peer satisfied the delta's
    /// governance position at send time, so it holds the full
    /// `StoredNamespaceEntry::Signed` op (with the encrypted group payload) —
    /// unlike an arbitrary namespace-topic subscriber, which may be a
    /// non-member holding only the `Opaque` skeleton and would serve nothing
    /// for the missing group op.
    pub(crate) fn governance_pending_source_peers(&self, context_id: &ContextId) -> Vec<PeerId> {
        let Some(entry) = self.governance_pending.get(context_id) else {
            return Vec::new();
        };
        let mut seen = std::collections::HashSet::new();
        entry
            .iter()
            .map(|d| d.source_peer)
            .filter(|p| seen.insert(*p))
            .collect()
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
        let time_evicted = before_count.saturating_sub(after_time_eviction);
        let count_evicted = after_time_eviction.saturating_sub(after_count_eviction);
        let memory_evicted = after_count_eviction.saturating_sub(self.blob_cache.len());
        if total_evicted > 0 {
            tracing::debug!(
                total_evicted,
                time_evicted,
                count_evicted,
                memory_evicted,
                remaining_count = self.blob_cache.len(),
                "Blob cache eviction completed"
            );
            // Bump per-reason eviction counters. Recorded after all three
            // eviction passes so each reason gets its share without double
            // counting (each pass strictly removes entries the next pass
            // would otherwise also see).
            crate::node_metrics::record_blob_cache_eviction("age", time_evicted as u64);
            crate::node_metrics::record_blob_cache_eviction("count", count_evicted as u64);
            crate::node_metrics::record_blob_cache_eviction("memory", memory_evicted as u64);
        }
    }
}

// Production implementation of the `SyncStateAccess` trait. Inverts the
// dependency: `sync/` consumes `&dyn SyncStateAccess` rather than
// reaching into `NodeState`'s fields/methods directly, which lets unit
// tests substitute a recording fake.
impl crate::sync::state_access::SyncStateAccess for NodeState {
    fn delta_store(&self, context_id: &ContextId) -> Option<DeltaStore> {
        self.delta_stores.get(context_id).map(|entry| entry.clone())
    }

    fn get_or_register_delta_store(
        &self,
        context_id: ContextId,
        factory: Box<dyn FnOnce() -> DeltaStore + Send>,
    ) -> (DeltaStore, bool) {
        // DashMap's `entry().or_insert_with()` runs the factory at most
        // once per `context_id`, under the shard write-lock. The
        // `was_newly_created` flag is updated inside that critical
        // section, so the value the caller sees reflects whether
        // *this thread* won the create race. Under contention exactly
        // one thread sees `true`; every other thread sees `false`. The
        // one-time setup at call sites (`load_persisted_deltas` after
        // a fresh store) is therefore guaranteed to run exactly once
        // per context across threads.
        let mut was_newly_created = false;
        let store = self
            .delta_stores
            .entry(context_id)
            .or_insert_with(|| {
                was_newly_created = true;
                factory()
            })
            .clone();
        (store, was_newly_created)
    }

    fn end_sync_session(
        &self,
        context_id: &ContextId,
    ) -> Option<Vec<calimero_node_primitives::delta_buffer::BufferedDelta>> {
        Self::end_sync_session(self, context_id)
    }

    fn cancel_sync_session(&self, context_id: &ContextId) {
        Self::cancel_sync_session(self, context_id)
    }

    fn peer_identities(&self, peer_id: &PeerId) -> Option<BTreeSet<PublicKey>> {
        self.peer_identities.get(peer_id).map(|entry| entry.clone())
    }

    fn cached_member_peers_for_group(
        &self,
        group: &ContextGroupId,
    ) -> Vec<(PeerId, GroupMemberRole)> {
        self.lock_peer_identity_cache()
            .members_for_group(group, now_unix_secs(), PEER_IDENTITY_TTL_SECS)
            .into_iter()
            .flat_map(|member| {
                let role = member.role;
                member
                    .peers
                    .into_iter()
                    .map(move |peer| (peer, role.clone()))
            })
            .collect()
    }

    fn reconcile_remaining_cooldown(&self, context_id: &ContextId) -> Option<(Duration, u32)> {
        crate::sync::reconcile_remaining_cooldown(&self.reconcile_attempts, context_id)
    }

    fn record_reconcile_success(&self, context_id: &ContextId) {
        crate::sync::record_reconcile_success(&self.reconcile_attempts, context_id);
    }

    fn record_reconcile_failure(&self, context_id: ContextId) -> u32 {
        crate::sync::record_reconcile_failure(&self.reconcile_attempts, context_id)
    }
}

#[cfg(test)]
mod tests {
    use calimero_node_primitives::delta_buffer::BufferedDelta;
    use calimero_primitives::hash::Hash;
    use calimero_storage::logical_clock::HybridTimestamp;

    use super::*;

    /// An observation carrying membership populates the durable cache so
    /// `cached_member_peers_for_group` returns the (peer, role); an
    /// observation with `None` membership does not.
    #[test]
    fn cached_member_peers_reflects_observed_membership() {
        use crate::sync::state_access::SyncStateAccess;

        let state = NodeState::new(false, NodeMode::Standard);
        let group = ContextGroupId::from([3u8; 32]);
        let identity = PublicKey::from([4u8; 32]);
        let peer = PeerId::random();

        state.observe_peer_identity(
            peer,
            identity,
            Some(ObservedMembership {
                group_id: group,
                role: GroupMemberRole::Admin,
            }),
        );
        assert_eq!(
            state.cached_member_peers_for_group(&group),
            vec![(peer, GroupMemberRole::Admin)]
        );

        // Membership-less observation (namespace path) doesn't reach the
        // per-group durable cache.
        state.observe_peer_identity(PeerId::random(), PublicKey::from([5u8; 32]), None);
        assert_eq!(
            state.cached_member_peers_for_group(&group),
            vec![(peer, GroupMemberRole::Admin)]
        );
    }

    fn buffered(id: u8, source_peer: PeerId) -> BufferedDelta {
        BufferedDelta {
            id: [id; 32],
            parents: vec![],
            hlc: HybridTimestamp::zero(),
            payload: vec![],
            nonce: [0u8; 12],
            author_id: PublicKey::from([0u8; 32]),
            root_hash: Hash::from([0u8; 32]),
            events: None,
            source_peer,
            key_id: [0u8; 32],
            governance_position: None,
            delta_signature: None,
            governance_drain_attempts: 0,
            producing_app_key: None,
        }
    }

    /// #2625: the backfill targets the peers that delivered the stuck deltas.
    /// They must be distinct (one stream per peer) and in first-seen order
    /// (try the earliest deliverer first).
    #[test]
    fn governance_pending_source_peers_dedups_and_preserves_order() {
        let state = NodeState::new(false, NodeMode::Standard);
        let ctx = ContextId::from([7u8; 32]);
        let p1 = PeerId::random();
        let p2 = PeerId::random();

        // Deliveries p1, p2, p1 → distinct first-seen order [p1, p2].
        state.buffer_governance_pending(ctx, buffered(1, p1));
        state.buffer_governance_pending(ctx, buffered(2, p2));
        state.buffer_governance_pending(ctx, buffered(3, p1));

        assert_eq!(state.governance_pending_source_peers(&ctx), vec![p1, p2]);
    }

    #[test]
    fn governance_pending_source_peers_empty_for_unknown_context() {
        let state = NodeState::new(false, NodeMode::Standard);
        assert!(state
            .governance_pending_source_peers(&ContextId::from([9u8; 32]))
            .is_empty());
    }
}
