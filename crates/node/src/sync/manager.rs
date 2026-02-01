//! Sync manager and orchestration.
//!
//! **Purpose**: Coordinates periodic syncs, selects peers, and delegates to protocols.
//! **Strategy**: Try delta sync first, fallback to state sync on failure.
//!
//! ## Merge Callbacks
//!
//! For hash-based incremental sync (comparing Merkle trees), we need CRDT merge logic:
//! - **Built-in CRDTs** (Counter, Map, etc.) are merged in the storage layer
//! - **Custom types** require WASM callbacks via `RuntimeMergeCallback`
//!
//! The `get_merge_callback()` method creates the appropriate callback for a context.

use std::collections::{hash_map, HashMap};
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use calimero_context_primitives::client::ContextClient;
use calimero_crypto::{Nonce, SharedKey};
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::sync::{
    InitPayload, MessagePayload, StreamMessage, TreeLeafData, TreeNode, TreeNodeChild,
};
use calimero_primitives::common::DIGEST_SIZE;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_runtime::merge_callback::RuntimeMergeCallback;
use calimero_storage::entities::Metadata;
use calimero_storage::index::EntityIndex;
use calimero_storage::store::Key as StorageKey;
use calimero_storage::WasmMergeCallback;
use eyre::bail;
use futures_util::stream::{self, FuturesUnordered};
use futures_util::{FutureExt, StreamExt};
use libp2p::gossipsub::{IdentTopic, TopicHash};
use libp2p::PeerId;
use rand::Rng;
use tokio::sync::mpsc;
use tokio::time::{self, timeout_at, Instant, MissedTickBehavior};
use tracing::{debug, error, info, warn};

use crate::utils::choose_stream;

use super::config::{StateSyncStrategy, SyncConfig};
use super::tracking::{SyncProtocol, SyncState};

/// Network synchronization manager.
///
/// Orchestrates sync protocols: full resync, delta sync, state sync.
#[derive(Debug)]
pub struct SyncManager {
    pub(crate) sync_config: SyncConfig,

    pub(super) node_client: NodeClient,
    pub(super) context_client: ContextClient,
    pub(crate) network_client: NetworkClient,
    pub(super) node_state: crate::NodeState,

    pub(super) ctx_sync_rx: Option<mpsc::Receiver<(Option<ContextId>, Option<PeerId>)>>,

    /// Prometheus metrics for sync operations.
    pub(super) metrics: super::metrics::SharedSyncMetrics,

    /// Cache of recently successful peers per context.
    pub(super) recent_peer_cache: super::peer_finder::SharedRecentPeerCache,

    /// Connection pool statistics for dial optimization.
    pub(super) dial_pool_stats: super::dial_tracker::SharedPoolStats,

    /// Connection state tracker for RTT-based peer selection.
    pub(super) connection_state: super::dial_tracker::SharedConnectionState,
}

impl Clone for SyncManager {
    fn clone(&self) -> Self {
        Self {
            sync_config: self.sync_config,
            node_client: self.node_client.clone(),
            context_client: self.context_client.clone(),
            network_client: self.network_client.clone(),
            node_state: self.node_state.clone(),
            ctx_sync_rx: None, // Receiver can't be cloned
            metrics: self.metrics.clone(),
            recent_peer_cache: self.recent_peer_cache.clone(),
            dial_pool_stats: self.dial_pool_stats.clone(),
            connection_state: self.connection_state.clone(),
        }
    }
}

impl SyncManager {
    pub(crate) fn new(
        sync_config: SyncConfig,
        node_client: NodeClient,
        context_client: ContextClient,
        network_client: NetworkClient,
        node_state: crate::NodeState,
        ctx_sync_rx: mpsc::Receiver<(Option<ContextId>, Option<PeerId>)>,
        metrics: super::metrics::SharedSyncMetrics,
    ) -> Self {
        Self {
            sync_config,
            node_client,
            context_client,
            network_client,
            node_state,
            ctx_sync_rx: Some(ctx_sync_rx),
            metrics,
            recent_peer_cache: super::peer_finder::new_recent_peer_cache(),
            dial_pool_stats: super::dial_tracker::new_pool_stats(),
            connection_state: super::dial_tracker::new_connection_state(),
        }
    }

    pub async fn start(mut self) {
        let mut next_sync = time::interval(self.sync_config.frequency);

        next_sync.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let mut state = HashMap::<_, SyncState>::new();

        let mut futs = FuturesUnordered::new();

        let metrics = self.metrics.clone();
        let advance = async |futs: &mut FuturesUnordered<_>,
                             state: &mut HashMap<_, SyncState>,
                             metrics: &super::metrics::SyncMetrics| {
            let (context_id, peer_id, start, result): (
                ContextId,
                PeerId,
                Instant,
                Result<Result<SyncProtocol, eyre::Error>, time::error::Elapsed>,
            ) = futs.next().await?;

            let now = Instant::now();
            let took = Instant::saturating_duration_since(&now, start);
            let duration_secs = took.as_secs_f64();

            let _ignored = state.entry(context_id).and_modify(|state| match result {
                Ok(Ok(protocol)) => {
                    state.on_success(peer_id, protocol);

                    // Record metrics
                    metrics.sync_duration.observe(duration_secs);
                    metrics.sync_successes.inc();

                    info!(
                        %context_id,
                        ?took,
                        duration_ms = format!("{:.2}", duration_secs * 1000.0),
                        ?protocol,
                        success_count = state.success_count,
                        "Sync finished successfully"
                    );
                }
                Ok(Err(ref err)) => {
                    state.on_failure(err.to_string());

                    // Record failure metrics
                    metrics.sync_duration.observe(duration_secs);
                    metrics.sync_failures.inc();

                    warn!(
                        %context_id,
                        ?took,
                        duration_ms = format!("{:.2}", duration_secs * 1000.0),
                        error = %err,
                        failure_count = state.failure_count(),
                        backoff_secs = state.backoff_delay().as_secs(),
                        "Sync failed, applying exponential backoff"
                    );
                }
                Err(ref timeout_err) => {
                    state.on_failure(timeout_err.to_string());

                    // Record timeout metrics
                    metrics.sync_duration.observe(duration_secs);
                    metrics.sync_failures.inc();

                    warn!(
                        %context_id,
                        ?took,
                        duration_ms = format!("{:.2}", duration_secs * 1000.0),
                        failure_count = state.failure_count(),
                        backoff_secs = state.backoff_delay().as_secs(),
                        "Sync timed out, applying exponential backoff"
                    );
                }
            });

            Some(())
        };

        let mut requested_ctx = None;
        let mut requested_peer = None;

        let Some(mut ctx_sync_rx) = self.ctx_sync_rx.take() else {
            error!("SyncManager can only be run once");

            return;
        };

        loop {
            tokio::select! {
                _ = next_sync.tick() => {
                    debug!("Performing interval sync");
                }
                Some(()) = async {
                    loop { advance(&mut futs, &mut state, &metrics).await? }
                } => {},
                Some((ctx, peer)) = ctx_sync_rx.recv() => {
                    info!(?ctx, ?peer, "Received sync request");

                    requested_ctx = ctx;
                    requested_peer = peer;

                    // CRITICAL FIX: Drain all other pending sync requests in the queue.
                    // When multiple contexts join rapidly (common in E2E tests), they all
                    // call sync() which queues requests in ctx_sync_rx. The old code only
                    // processed ONE request per loop iteration, leaving contexts 2-N queued
                    // indefinitely. This caused those contexts to never sync and remain
                    // with dag_heads=[] and Uninitialized errors.
                    //
                    // Solution: Use try_recv() to drain all buffered requests immediately,
                    // then trigger a full sync that will process all contexts.
                    let mut drained_count = 0;
                    while ctx_sync_rx.try_recv().is_ok() {
                        drained_count += 1;
                    }

                    if drained_count > 0 {
                        info!(drained_count, "Drained additional sync requests from queue, will sync all contexts");
                        // Clear requested_ctx to force syncing ALL contexts
                        // This ensures newly-joined contexts get synced even if they weren't first in queue
                        requested_ctx = None;
                        requested_peer = None;
                    }
                }
            }

            let requested_ctx = requested_ctx.take();
            let requested_peer = requested_peer.take();

            let contexts = requested_ctx
                .is_none()
                .then(|| self.context_client.get_context_ids(None));

            let contexts = stream::iter(requested_ctx)
                .map(Ok)
                .chain(stream::iter(contexts).flatten());

            let mut contexts = pin!(contexts);

            while let Some(context_id) = contexts.next().await {
                let context_id = match context_id {
                    Ok(context_id) => context_id,
                    Err(err) => {
                        error!(%err, "Failed reading context id to sync");
                        continue;
                    }
                };

                match state.entry(context_id) {
                    hash_map::Entry::Occupied(state) => {
                        let state = state.into_mut();

                        let Some(last_sync) = state.last_sync() else {
                            debug!(
                                %context_id,
                                "Sync already in progress"
                            );

                            continue;
                        };

                        let minimum = self.sync_config.interval;
                        let time_since = last_sync.elapsed();

                        if time_since < minimum {
                            if requested_ctx.is_none() {
                                debug!(%context_id, ?time_since, ?minimum, "Skipping sync, last one was too recent");

                                continue;
                            }

                            debug!(%context_id, ?time_since, ?minimum, "Force syncing despite recency, due to explicit request");
                        }

                        let _ignored = state.take_last_sync();
                    }
                    hash_map::Entry::Vacant(state) => {
                        info!(
                            %context_id,
                            "Syncing for the first time"
                        );

                        let mut new_state = SyncState::new();
                        new_state.start();
                        let _ignored = state.insert(new_state);
                    }
                };

                info!(%context_id, "Scheduled sync");

                let start = Instant::now();
                let Some(deadline) = start.checked_add(self.sync_config.timeout) else {
                    error!(
                        ?start,
                        timeout=?self.sync_config.timeout,
                        "Unable to determine when to timeout sync procedure"
                    );

                    // if we can't determine the sync deadline, this is a hard error
                    // we intentionally want to exit the sync loop
                    return;
                };

                let fut = timeout_at(
                    deadline,
                    self.perform_interval_sync(context_id, requested_peer),
                )
                .map(move |res| {
                    // Extract peer_id from result or use placeholder
                    let peer_id = res
                        .as_ref()
                        .ok()
                        .and_then(|r| r.as_ref().ok())
                        .map(|(p, _)| *p)
                        .unwrap_or(PeerId::random());
                    (
                        context_id,
                        peer_id,
                        start,
                        res.map(|r| r.map(|(_, proto)| proto)),
                    )
                });

                futs.push(fut);

                if futs.len() >= self.sync_config.max_concurrent {
                    let _ignored = advance(&mut futs, &mut state, &metrics).await;
                }
            }
        }
    }

    async fn perform_interval_sync(
        &self,
        context_id: ContextId,
        peer_id: Option<PeerId>,
    ) -> eyre::Result<(PeerId, SyncProtocol)> {
        use super::peer_finder::{PeerFindResult, PeerFindTracker, SourceBreakdown};

        if let Some(peer_id) = peer_id {
            return self.initiate_sync(context_id, peer_id).await;
        }

        // ========================================================================
        // PEER FINDING INSTRUMENTATION (separates finding from connecting)
        // ========================================================================
        let mut tracker = PeerFindTracker::new();

        // ========================================================================
        // PHASE 0: MESH WAIT (NOT peer finding - this is network formation)
        // ========================================================================
        // CRITICAL FIX: Wait for gossipsub mesh to form after restart
        //
        // After a node restarts or joins a context, gossipsub needs time to:
        // 1. Re-subscribe to topics
        // 2. Exchange GRAFT messages with peers
        // 3. Form the mesh
        //
        // This can take 10-20 seconds depending on heartbeat intervals.
        // We use a configurable timeout with periodic checks.
        //
        // MESH RECOVERY FIX: If mesh doesn't form after initial wait, force a
        // re-subscribe to trigger gossipsub to re-negotiate the mesh. This handles
        // asymmetric mesh state that can occur after node restarts.
        let mesh_timeout = self.sync_config.mesh_formation_timeout;
        let check_interval = self.sync_config.mesh_formation_check_interval;
        let deadline = time::Instant::now() + mesh_timeout;

        let mut peers;
        let mut attempt = 0;
        let mut resubscribed = false;

        loop {
            attempt += 1;
            peers = self
                .network_client
                .mesh_peers(TopicHash::from_raw(context_id))
                .await;

            if !peers.is_empty() {
                if attempt > 1 {
                    info!(
                        %context_id,
                        attempt,
                        peer_count = peers.len(),
                        elapsed_ms = (mesh_timeout.as_millis() as u64).saturating_sub(
                            (deadline - time::Instant::now()).as_millis() as u64
                        ),
                        resubscribed,
                        "Gossipsub mesh formed successfully after waiting"
                    );
                }
                break;
            }

            if time::Instant::now() >= deadline {
                warn!(
                    %context_id,
                    attempts = attempt,
                    timeout_secs = mesh_timeout.as_secs(),
                    resubscribed,
                    "Gossipsub mesh failed to form within timeout"
                );
                break;
            }

            // MESH RECOVERY: If no mesh after 5 attempts (~5s), force re-subscribe
            // This fixes asymmetric mesh state that can occur when a node restarts
            // and the remote peer's gossipsub still thinks the old connection is valid.
            if attempt == 5 && !resubscribed {
                info!(
                    %context_id,
                    "Forcing re-subscribe to trigger mesh re-negotiation"
                );
                // Unsubscribe and re-subscribe to force gossipsub to re-GRAFT
                let topic = IdentTopic::new(context_id);
                if let Err(e) = self.network_client.unsubscribe(topic.clone()).await {
                    debug!(%context_id, error = %e, "Unsubscribe failed (may already be unsubscribed)");
                }
                time::sleep(Duration::from_millis(100)).await;
                if let Err(e) = self.network_client.subscribe(topic).await {
                    warn!(%context_id, error = %e, "Re-subscribe failed");
                }
                resubscribed = true;
            }

            if attempt == 1 {
                debug!(
                    %context_id,
                    timeout_secs = mesh_timeout.as_secs(),
                    "No peers in mesh yet, waiting for gossipsub mesh formation..."
                );
            } else if attempt % 5 == 0 {
                debug!(
                    %context_id,
                    attempt,
                    remaining_secs = (deadline - time::Instant::now()).as_secs(),
                    "Still waiting for gossipsub mesh to form..."
                );
            }

            time::sleep(check_interval).await;
        }

        // Mesh wait is complete - NOW start peer finding timing
        // ========================================================================
        // PHASE 1: CANDIDATE LOOKUP (peer finding starts here)
        // ========================================================================
        tracker.start_candidate_lookup();

        // The peers we already have from mesh wait are our candidates
        // In the future, we could also query routing table, address book, etc.
        let strategy = self.sync_config.peer_find_strategy;
        let context_id_bytes: [u8; 32] = *context_id.as_ref();

        // Get candidates from all sources based on strategy
        let (all_candidates, source_breakdown) = {
            let cache = self.recent_peer_cache.read().unwrap();
            let recent = cache.get_recent(context_id_bytes);
            let from_recent = recent.len();
            let from_mesh = peers.len();

            // Combine sources based on strategy
            let candidates = match strategy {
                super::peer_finder::PeerFindStrategy::RecentFirst => {
                    let mut all = recent;
                    for p in &peers {
                        if !all.contains(p) {
                            all.push(*p);
                        }
                    }
                    all
                }
                super::peer_finder::PeerFindStrategy::ParallelFind => {
                    let mut all = recent;
                    for p in &peers {
                        if !all.contains(p) {
                            all.push(*p);
                        }
                    }
                    all
                }
                _ => peers.clone(),
            };

            (
                candidates,
                SourceBreakdown {
                    mesh: from_mesh,
                    recent: from_recent,
                    book: 0,
                    routing: 0,
                },
            )
        };

        // End candidate lookup, start filtering
        tracker.end_candidate_lookup(&all_candidates, source_breakdown);

        if all_candidates.is_empty() {
            tracker.mark_failed(PeerFindResult::NoCandidates);
            let _ = tracker.finish(&context_id.to_string());

            bail!(
                "No peers to sync with for context {} (mesh failed to form after {}s)",
                context_id,
                mesh_timeout.as_secs()
            );
        }

        // ========================================================================
        // PHASE 2: FILTERING (apply quality filters)
        // ========================================================================
        let backoff_duration = Duration::from_secs(30);

        let filtered_peers: Vec<PeerId> = {
            let cache = self.recent_peer_cache.read().unwrap();
            match strategy {
                super::peer_finder::PeerFindStrategy::HealthFiltered => {
                    cache.filter_viable(&all_candidates, backoff_duration)
                }
                _ => all_candidates.to_vec(),
            }
        };

        // End filtering, start selection
        tracker.end_filtering(filtered_peers.len());

        if filtered_peers.is_empty() {
            tracker.mark_failed(PeerFindResult::AllFiltered);
            let _ = tracker.finish(&context_id.to_string());

            bail!(
                "All {} peer candidates filtered out for context {}",
                all_candidates.len(),
                context_id
            );
        }

        // ========================================================================
        // PHASE 3: SELECTION (pick the final peer)
        // ========================================================================
        // Optimization: Sort peers to prefer already-connected ones
        // This reduces dial latency by favoring connection reuse
        let sorted_peers = {
            let conn_state = self.connection_state.read().unwrap();
            let mut peers_with_score: Vec<_> = filtered_peers
                .iter()
                .map(|p| {
                    // Score: connected peers first, then by RTT
                    let is_connected = conn_state.is_likely_connected(p);
                    let rtt = conn_state
                        .get(p)
                        .and_then(|s| s.rtt_estimate_ms)
                        .unwrap_or(f64::MAX);
                    // Lower score = higher priority (connected=0, disconnected=1000)
                    let score = if is_connected { rtt } else { 1000.0 + rtt };
                    (*p, score)
                })
                .collect();
            peers_with_score
                .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            peers_with_score
                .into_iter()
                .map(|(p, _)| p)
                .collect::<Vec<_>>()
        };

        let (selected_peers, peer_source) = {
            let cache = self.recent_peer_cache.read().unwrap();
            cache.select_by_strategy(strategy, context_id_bytes, &sorted_peers, backoff_duration)
        };

        debug!(
            %context_id,
            %strategy,
            %peer_source,
            raw_candidates = all_candidates.len(),
            filtered = filtered_peers.len(),
            selected = selected_peers.len(),
            "Peer finding phases complete (finding only, no dial)"
        );

        // Check if we're uninitialized
        let context = self
            .context_client
            .get_context(&context_id)?
            .ok_or_else(|| eyre::eyre!("Context not found: {}", context_id))?;

        let is_uninitialized = *context.root_hash == [0; 32];

        if is_uninitialized {
            // When uninitialized, we need to bootstrap from a peer that HAS data
            // Trying random peers can result in querying other uninitialized nodes
            info!(
                %context_id,
                peer_count = selected_peers.len(),
                "Node is uninitialized, selecting peer with state for bootstrapping"
            );

            // Try to find a peer with actual state
            match self.find_peer_with_state(context_id, &selected_peers).await {
                Ok(peer_id) => {
                    info!(%context_id, %peer_id, "Found peer with state, syncing from them");

                    // Check if this peer was in recent cache
                    let was_recent = {
                        let cache = self.recent_peer_cache.read().unwrap();
                        cache.get_recent(context_id_bytes).contains(&peer_id)
                    };

                    // End selection phase - PEER FINDING COMPLETE (no dial time included)
                    tracker.end_selection(peer_source, was_recent);
                    let phases = tracker.finish(&context_id.to_string());

                    // ========================================================
                    // DIAL PHASE (separate from peer finding)
                    // ========================================================
                    let dial_start = Instant::now();
                    let result = self.initiate_sync(context_id, peer_id).await;
                    let dial_ms = dial_start.elapsed().as_secs_f64() * 1000.0;

                    info!(
                        %context_id,
                        %peer_id,
                        time_to_viable_peer_ms = %format!("{:.2}", phases.time_to_viable_peer_ms()),
                        dial_ms = %format!("{:.2}", dial_ms),
                        "PEER_DIAL_TIMING"
                    );

                    // Record success/failure in cache
                    if result.is_ok() {
                        let mut cache = self.recent_peer_cache.write().unwrap();
                        cache.record_success(context_id_bytes, peer_id, peer_source);
                    } else {
                        let mut cache = self.recent_peer_cache.write().unwrap();
                        cache.record_failure(peer_id);
                    }

                    return result;
                }
                Err(e) => {
                    warn!(%context_id, error = %e, "Failed to find peer with state, falling back to strategy selection");
                    // Fall through to strategy-based selection
                }
            }
        }

        // Normal sync: use PARALLEL DIALING for better P99 latency
        debug!(%context_id, %strategy, "Using parallel dialing for sync");

        // End selection phase - PEER FINDING COMPLETE (no dial time included)
        let was_recent = {
            let cache = self.recent_peer_cache.read().unwrap();
            selected_peers
                .first()
                .map(|p| cache.get_recent(context_id_bytes).contains(p))
                .unwrap_or(false)
        };
        tracker.end_selection(peer_source, was_recent);
        let phases = tracker.into_phases();
        phases.log(&context_id.to_string());

        // ========================================================
        // TRUE PARALLEL DIAL PHASE (using FuturesUnordered)
        // ========================================================
        use super::dial_tracker::{DialResult, ParallelDialConfig, ParallelDialTracker};

        let parallel_config = ParallelDialConfig {
            max_concurrent: 3.min(selected_peers.len()), // Dial up to 3 peers at once
            dial_timeout_ms: 5000,
            cancel_on_success: true,
        };

        let mut parallel_tracker = ParallelDialTracker::new(parallel_config);
        let dial_start = Instant::now();

        // Select peers to dial in parallel
        let peers_to_dial: Vec<_> = selected_peers
            .iter()
            .take(parallel_config.max_concurrent)
            .copied()
            .collect();

        info!(
            %context_id,
            peer_count = peers_to_dial.len(),
            "Starting TRUE parallel dial to peers"
        );

        // Create a FuturesUnordered to race all dial attempts concurrently
        let mut dial_futures: FuturesUnordered<_> = peers_to_dial
            .iter()
            .map(|&peer_id| {
                let attempt_start = Instant::now();
                async move {
                    let result = self.initiate_sync(context_id, peer_id).await;
                    let dial_ms = attempt_start.elapsed().as_secs_f64() * 1000.0;
                    (peer_id, result, dial_ms)
                }
            })
            .collect();

        // Race all dial attempts - first success wins, others are dropped
        let mut last_error = None;
        let mut attempts = 0u32;

        while let Some((peer_id, result, dial_ms)) = dial_futures.next().await {
            attempts += 1;

            match result {
                Ok(sync_result) => {
                    // SUCCESS! First successful dial wins
                    parallel_tracker.record(peer_id, DialResult::Success, dial_ms);

                    // Drop remaining futures (they'll be cancelled)
                    drop(dial_futures);

                    let parallel_result = parallel_tracker.finish(&context_id.to_string());

                    info!(
                        %context_id,
                        %peer_id,
                        time_to_viable_peer_ms = %format!("{:.2}", phases.time_to_viable_peer_ms()),
                        dial_ms = %format!("{:.2}", dial_ms),
                        total_attempts = parallel_result.attempts,
                        concurrent_cancelled = peers_to_dial.len().saturating_sub(attempts as usize),
                        result = "success",
                        "TRUE_PARALLEL_DIAL_SUCCESS"
                    );

                    // Record success in cache
                    {
                        let mut cache = self.recent_peer_cache.write().unwrap();
                        cache.record_success(context_id_bytes, peer_id, peer_source);
                    }

                    return Ok(sync_result);
                }
                Err(e) => {
                    parallel_tracker.record(peer_id, DialResult::Error, dial_ms);

                    debug!(
                        %context_id,
                        %peer_id,
                        dial_ms = %format!("{:.2}", dial_ms),
                        error = %e,
                        attempt = attempts,
                        remaining = dial_futures.len(),
                        "Parallel dial attempt failed, waiting for others"
                    );

                    // Record failure in cache
                    {
                        let mut cache = self.recent_peer_cache.write().unwrap();
                        cache.record_failure(peer_id);
                    }

                    last_error = Some(e);
                    // Continue to next future (others are still racing)
                }
            }
        }

        // All parallel attempts failed
        let total_dial_ms = dial_start.elapsed().as_secs_f64() * 1000.0;
        let parallel_result = parallel_tracker.finish(&context_id.to_string());

        warn!(
            %context_id,
            attempts = parallel_result.attempts,
            total_dial_ms = %format!("{:.2}", total_dial_ms),
            "All parallel dial attempts failed"
        );

        match last_error {
            Some(e) => Err(e),
            None => bail!("Failed to sync with any peer for context {}", context_id),
        }
    }

    /// Find a peer that has state (non-zero root_hash and non-empty DAG heads)
    ///
    /// This is critical for bootstrapping newly joined nodes. Without this,
    /// uninitialized nodes may query other uninitialized nodes, resulting in
    /// all nodes remaining uninitialized.
    async fn find_peer_with_state(
        &self,
        context_id: ContextId,
        peers: &[PeerId],
    ) -> eyre::Result<PeerId> {
        use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};

        // Get our identity for handshake
        let identities = self
            .context_client
            .get_context_members(&context_id, Some(true));

        let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
            .await
            .transpose()?
        else {
            bail!("no owned identities found for context: {}", context_id);
        };

        // Query peers to find one with state
        for peer_id in peers {
            debug!(%context_id, %peer_id, "Querying peer for state");

            // Try to open stream and request DAG heads
            let stream_result = self.network_client.open_stream(*peer_id).await;
            let mut stream = match stream_result {
                Ok(s) => s,
                Err(e) => {
                    debug!(%context_id, %peer_id, error = %e, "Failed to open stream to peer");
                    continue;
                }
            };

            // Send DAG heads request
            let request_msg = StreamMessage::Init {
                context_id,
                party_id: our_identity,
                payload: InitPayload::DagHeadsRequest { context_id },
                next_nonce: {
                    use rand::Rng;
                    rand::thread_rng().gen()
                },
            };

            if let Err(e) = self.send(&mut stream, &request_msg, None).await {
                debug!(%context_id, %peer_id, error = %e, "Failed to send DAG heads request");
                continue;
            }

            // Receive response with short timeout
            let timeout_budget = self.sync_config.timeout / 6;
            let response = match super::stream::recv(&mut stream, None, timeout_budget).await {
                Ok(Some(resp)) => resp,
                Ok(None) => {
                    debug!(%context_id, %peer_id, "No response from peer");
                    continue;
                }
                Err(e) => {
                    debug!(%context_id, %peer_id, error = %e, "Failed to receive response");
                    continue;
                }
            };

            // Check if peer has state
            if let StreamMessage::Message {
                payload:
                    MessagePayload::DagHeadsResponse {
                        dag_heads,
                        root_hash,
                    },
                ..
            } = response
            {
                // Peer has state if root_hash is not zeros
                // (even if dag_heads is empty due to migration/legacy contexts)
                let has_state = *root_hash != [0; 32];

                debug!(
                    %context_id,
                    %peer_id,
                    heads_count = dag_heads.len(),
                    %root_hash,
                    has_state,
                    "Received DAG heads from peer"
                );

                if has_state {
                    info!(
                        %context_id,
                        %peer_id,
                        heads_count = dag_heads.len(),
                        %root_hash,
                        "Found peer with state for bootstrapping"
                    );
                    return Ok(*peer_id);
                }
            }
        }

        bail!("No peers with state found for context {}", context_id)
    }

    async fn initiate_sync(
        &self,
        context_id: ContextId,
        peer_id: PeerId,
    ) -> eyre::Result<(PeerId, SyncProtocol)> {
        let start = Instant::now();

        info!(%context_id, %peer_id, "Attempting to sync with peer");

        let protocol = self.initiate_sync_inner(context_id, peer_id).await?;

        let took = start.elapsed();

        info!(%context_id, %peer_id, ?took, ?protocol, "Sync with peer completed successfully");

        Ok((peer_id, protocol))
    }

    /// Sends a message over the stream (delegates to stream module).
    pub(super) async fn send(
        &self,
        stream: &mut Stream,
        message: &StreamMessage<'_>,
        shared_key: Option<(SharedKey, Nonce)>,
    ) -> eyre::Result<()> {
        super::stream::send(stream, message, shared_key).await
    }

    /// Receives a message from the stream (delegates to stream module).
    pub(super) async fn recv(
        &self,
        stream: &mut Stream,
        shared_key: Option<(SharedKey, Nonce)>,
    ) -> eyre::Result<Option<StreamMessage<'static>>> {
        let budget = self.sync_config.timeout / 3;
        super::stream::recv(stream, shared_key, budget).await
    }

    /// Create a merge callback for hash-based incremental sync.
    ///
    /// This callback bridges storage-layer tree comparison with WASM merge logic:
    /// - Built-in CRDTs (Counter, Map, etc.) are merged directly in storage
    /// - Custom types call into WASM via the registry
    ///
    /// # Usage
    ///
    /// ```ignore
    /// let callback = self.get_merge_callback();
    /// let actions = calimero_storage::interface::compare_trees_with_callback(
    ///     remote_data,
    ///     index,
    ///     Some(&*callback),
    /// )?;
    /// ```
    ///
    /// # Note
    ///
    /// Used by hash-based incremental sync (tree sync strategies).
    #[must_use]
    pub(super) fn get_merge_callback(&self) -> Arc<dyn WasmMergeCallback> {
        // RuntimeMergeCallback uses the global type registry to dispatch merge calls
        // For custom types, it looks up the merge function by type name
        Arc::new(RuntimeMergeCallback::new())
    }

    /// Initiate sync protocol negotiation with a peer.
    ///
    /// Sends our capabilities and state info, receives peer's response with
    /// negotiated protocol. This determines which sync strategy to use.
    ///
    /// # Returns
    ///
    /// The negotiated protocol and peer's state info, or error if negotiation fails.
    pub(super) async fn initiate_sync_handshake(
        &self,
        context: &calimero_primitives::context::Context,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<calimero_node_primitives::sync_protocol::SyncHandshakeResponse> {
        use calimero_node_primitives::sync_protocol::{SyncCapabilities, SyncHandshake};
        use rand::thread_rng;

        let our_nonce = thread_rng().gen::<Nonce>();

        // Build our handshake with capabilities and current state
        let handshake = SyncHandshake {
            capabilities: SyncCapabilities::full(),
            root_hash: context.root_hash,
            dag_heads: context.dag_heads.clone(),
            entity_count: 0, // TODO: Get actual entity count from storage
        };

        info!(
            context_id = %context.id,
            our_root_hash = %context.root_hash,
            dag_heads = context.dag_heads.len(),
            "Sending sync handshake"
        );

        // Send handshake
        self.send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::SyncHandshake { handshake },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        // Wait for response
        let Some(response_msg) = self.recv(stream, None).await? else {
            bail!("Connection closed while awaiting sync handshake response");
        };

        // Parse response
        let response = match response_msg {
            StreamMessage::Message {
                payload: MessagePayload::SyncHandshakeResponse { response },
                ..
            } => response,
            unexpected => {
                bail!("Unexpected message during handshake: {:?}", unexpected);
            }
        };

        info!(
            context_id = %context.id,
            negotiated_protocol = ?response.negotiated_protocol,
            peer_root_hash = %response.root_hash,
            peer_entity_count = response.entity_count,
            "Received sync handshake response"
        );

        Ok(response)
    }

    /// Execute tree-based sync using the configured strategy and merge callback.
    ///
    /// This is the main entry point for hash-based incremental sync (HybridSync).
    /// It selects the optimal strategy based on configuration and tree characteristics,
    /// then executes the sync using CRDT merge semantics via `get_merge_callback()`.
    ///
    /// The merge callback is obtained internally by each sync strategy method, so
    /// callers don't need to pass it explicitly.
    pub(super) async fn handle_tree_sync_with_callback(
        &self,
        context_id: ContextId,
        context: &calimero_primitives::context::Context,
        peer_id: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<Option<SyncProtocol>> {
        // Get local state info for strategy selection
        let store_handle = self.context_client.datastore_handle();
        let local_keys = super::snapshot::get_entity_keys(&store_handle, context_id)?;
        let local_entity_count = local_keys.len();
        let local_has_data = local_entity_count > 0;

        // Estimate remote entity count (from handshake, or assume similar)
        let remote_entity_count = local_entity_count; // TODO: Use handshake.entity_count

        // Select strategy
        let strategy = self.select_state_sync_strategy(
            context_id,
            local_has_data,
            local_entity_count,
            remote_entity_count,
            2,  // tree_depth estimate
            10, // child_count estimate
        );

        info!(
            %context_id,
            %peer_id,
            ?strategy,
            local_entity_count,
            "Executing tree sync with strategy"
        );

        // Get root hashes for tree sync methods
        let local_root_hash = context.root_hash;
        // For remote root hash, we'd ideally get this from handshake, but for now use local
        // as the tree sync methods will handle the actual comparison
        let remote_root_hash = local_root_hash; // Will be updated during actual sync

        // Execute based on selected strategy
        let result = match strategy {
            StateSyncStrategy::Snapshot | StateSyncStrategy::CompressedSnapshot => {
                // Full snapshot sync
                self.request_dag_heads_and_sync(context_id, peer_id, our_identity, stream)
                    .await
                    .map(Some)?
            }
            StateSyncStrategy::BloomFilter {
                false_positive_rate,
            } => {
                // Bloom filter sync for large trees
                self.bloom_filter_sync(
                    context_id,
                    peer_id,
                    our_identity,
                    stream,
                    false_positive_rate,
                )
                .await
                .map(Some)?
            }
            StateSyncStrategy::HashComparison => {
                // Recursive hash comparison
                self.hash_comparison_sync(
                    context_id,
                    peer_id,
                    our_identity,
                    stream,
                    local_root_hash,
                    remote_root_hash,
                )
                .await
                .map(Some)?
            }
            StateSyncStrategy::SubtreePrefetch { max_depth } => {
                // Subtree prefetch for deep trees
                self.subtree_prefetch_sync(
                    context_id,
                    peer_id,
                    our_identity,
                    stream,
                    local_root_hash,
                    remote_root_hash,
                    max_depth,
                )
                .await
                .map(Some)?
            }
            StateSyncStrategy::LevelWise { max_depth } => {
                // Level-wise for wide shallow trees
                self.level_wise_sync(
                    context_id,
                    peer_id,
                    our_identity,
                    stream,
                    local_root_hash,
                    remote_root_hash,
                    max_depth,
                )
                .await
                .map(Some)?
            }
            StateSyncStrategy::Adaptive => {
                // Adaptive: choose based on characteristics
                if local_entity_count > 1000 {
                    self.bloom_filter_sync(context_id, peer_id, our_identity, stream, 0.01)
                        .await
                        .map(Some)?
                } else {
                    self.hash_comparison_sync(
                        context_id,
                        peer_id,
                        our_identity,
                        stream,
                        local_root_hash,
                        remote_root_hash,
                    )
                    .await
                    .map(Some)?
                }
            }
        };

        // TODO: When merge is needed, use merge_callback to resolve CRDT conflicts
        // Currently, entity application in tree_sync.rs uses direct PUT, but
        // for proper CRDT semantics, we should:
        // 1. Read local value
        // 2. Call merge_callback.merge_custom(type_name, local, remote)
        // 3. Write merged result
        //
        // This requires exposing entity type metadata in storage.

        Ok(result)
    }

    /// Select the state sync strategy to use for Merkle tree comparison.
    ///
    /// If the configured strategy is `Adaptive`, this method analyzes the tree
    /// characteristics and selects the optimal protocol. Otherwise, it uses
    /// the configured strategy directly.
    ///
    /// Returns the selected strategy and logs the selection decision.
    #[must_use]
    pub(super) fn select_state_sync_strategy(
        &self,
        context_id: ContextId,
        local_has_data: bool,
        local_entity_count: usize,
        remote_entity_count: usize,
        tree_depth: usize,
        child_count: usize,
    ) -> StateSyncStrategy {
        let configured = self.sync_config.state_sync_strategy;

        let mut selected = if configured.is_adaptive() {
            StateSyncStrategy::choose_protocol(
                local_has_data,
                local_entity_count,
                remote_entity_count,
                tree_depth,
                child_count,
            )
        } else {
            configured
        };

        // ========================================================
        // SAFETY CHECK: Never use Snapshot on initialized nodes!
        // This would overwrite local changes. Force HashComparison instead.
        // ========================================================
        if local_has_data {
            match selected {
                StateSyncStrategy::Snapshot | StateSyncStrategy::CompressedSnapshot => {
                    warn!(
                        %context_id,
                        configured = %configured,
                        "SAFETY: Snapshot strategy blocked for initialized node - using HashComparison to preserve local data"
                    );
                    selected = StateSyncStrategy::HashComparison;
                }
                _ => {}
            }
        }

        // Log strategy selection for observability
        info!(
            %context_id,
            configured = %configured,
            selected = %selected,
            local_has_data,
            local_entity_count,
            remote_entity_count,
            tree_depth,
            child_count,
            "Selected state sync strategy"
        );

        selected
    }

    /// Get blob ID and application config from application or context config
    async fn get_blob_info(
        &self,
        context_id: &ContextId,
        application: &Option<calimero_primitives::application::Application>,
    ) -> eyre::Result<(
        calimero_primitives::blobs::BlobId,
        Option<calimero_primitives::application::Application>,
    )> {
        if let Some(ref app) = application {
            Ok((app.blob.bytecode, None))
        } else {
            // Application not found - get blob_id from context config
            let context_config = self
                .context_client
                .context_config(context_id)?
                .ok_or_else(|| eyre::eyre!("context config not found"))?;
            let external_client = self
                .context_client
                .external_client(context_id, &context_config)?;
            let config_client = external_client.config();
            let app_config = config_client.application().await?;
            Ok((app_config.blob.bytecode, Some(app_config)))
        }
    }

    /// Get application size from application, cached config, or context config
    async fn get_application_size(
        &self,
        context_id: &ContextId,
        application: &Option<calimero_primitives::application::Application>,
        app_config_opt: &Option<calimero_primitives::application::Application>,
    ) -> eyre::Result<u64> {
        if let Some(ref app) = application {
            Ok(app.size)
        } else if let Some(ref app_config) = app_config_opt {
            Ok(app_config.size)
        } else {
            let context_config = self
                .context_client
                .context_config(context_id)?
                .ok_or_else(|| eyre::eyre!("context config not found"))?;
            let external_client = self
                .context_client
                .external_client(context_id, &context_config)?;
            let config_client = external_client.config();
            let app_config = config_client.application().await?;
            Ok(app_config.size)
        }
    }

    /// Get application source from cached config or context config
    async fn get_application_source(
        &self,
        context_id: &ContextId,
        app_config_opt: &Option<calimero_primitives::application::Application>,
    ) -> eyre::Result<calimero_primitives::application::ApplicationSource> {
        if let Some(ref app_config) = app_config_opt {
            Ok(app_config.source.clone())
        } else {
            let context_config = self
                .context_client
                .context_config(context_id)?
                .ok_or_else(|| eyre::eyre!("context config not found"))?;
            let external_client = self
                .context_client
                .external_client(context_id, &context_config)?;
            let config_client = external_client.config();
            let app_config = config_client.application().await?;
            Ok(app_config.source.clone())
        }
    }

    /// Install bundle application after blob sharing completes.
    ///
    /// Returns `Some(installed_application)` if a bundle was installed,
    /// `None` otherwise. Updates `context.application_id` if the installed
    /// ApplicationId differs from the context's ApplicationId.
    async fn install_bundle_after_blob_sharing(
        &self,
        context_id: &ContextId,
        blob_id: &calimero_primitives::blobs::BlobId,
        app_config_opt: &Option<calimero_primitives::application::Application>,
        context: &mut calimero_primitives::context::Context,
        application: &mut Option<calimero_primitives::application::Application>,
    ) -> eyre::Result<()> {
        // Only proceed if blob is now available locally
        if !self.node_client.has_blob(blob_id)? {
            return Ok(());
        }

        // Check if blob is a bundle
        let Some(blob_bytes) = self.node_client.get_blob_bytes(blob_id, None).await? else {
            return Ok(());
        };

        // Wrap blocking I/O in spawn_blocking to avoid blocking async runtime
        let blob_bytes_clone = blob_bytes.clone();
        let is_bundle =
            tokio::task::spawn_blocking(move || NodeClient::is_bundle_blob(&blob_bytes_clone))
                .await?;

        if !is_bundle {
            return Ok(());
        }

        // Get source from context config (use cached if available, otherwise fetch)
        let source = self
            .get_application_source(context_id, app_config_opt)
            .await?;

        // Install bundle
        let installed_app_id = self
            .node_client
            .install_application_from_bundle_blob(blob_id, &source)
            .await
            .map_err(|e| {
                eyre::eyre!(
                    "Failed to install bundle application from blob {}: {}",
                    blob_id,
                    e
                )
            })?;

        // Verify installation succeeded by fetching the installed application
        let installed_application = self
            .node_client
            .get_application(&installed_app_id)
            .map_err(|e| {
                eyre::eyre!(
                    "Failed to verify bundle installation for application {}: {}",
                    installed_app_id,
                    e
                )
            })?;

        let Some(installed_application) = installed_application else {
            bail!(
                "Bundle installation reported success but application {} is not retrievable",
                installed_app_id
            );
        };

        // Check if the installed ApplicationId matches the context's ApplicationId
        if installed_app_id != context.application_id {
            warn!(
                installed_app_id = %installed_app_id,
                context_app_id = %context.application_id,
                "Installed application ID does not match context application ID, updating to installed ID"
            );
            // Update context with the installed application ID for consistency
            context.application_id = installed_app_id;

            // Persist the ApplicationId change to the database
            // This is critical: if we don't persist, the old ApplicationId will be
            // used on node restart, causing application lookup failures
            self.context_client
                .update_context_application_id(context_id, installed_app_id)
                .map_err(|e| {
                    eyre::eyre!(
                        "Failed to persist ApplicationId update for context {}: {}",
                        context_id,
                        e
                    )
                })?;

            debug!(
                %context_id,
                installed_app_id = %installed_app_id,
                "Persisted ApplicationId update to database"
            );
        }

        // Use the verified installed application
        *application = Some(installed_application);

        Ok(())
    }

    /// Handle DAG synchronization for uninitialized nodes or nodes with incomplete DAGs
    async fn handle_dag_sync(
        &self,
        context_id: ContextId,
        context: &calimero_primitives::context::Context,
        chosen_peer: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<Option<SyncProtocol>> {
        let is_uninitialized = *context.root_hash == [0; 32];

        // Check for incomplete sync from a previous run (crash recovery)
        let has_incomplete_sync = self.check_sync_in_progress(context_id)?.is_some();
        if has_incomplete_sync {
            warn!(
                %context_id,
                "Detected incomplete snapshot sync from previous run, forcing re-sync"
            );
        }

        if is_uninitialized || has_incomplete_sync {
            let strategy = self.sync_config.fresh_node_strategy;
            info!(
                %context_id,
                %chosen_peer,
                is_uninitialized,
                has_incomplete_sync,
                %strategy,
                "Node needs sync, checking peer state"
            );

            // Query peer's state to decide sync strategy
            let peer_state = self
                .query_peer_dag_state(context_id, chosen_peer, our_identity, stream)
                .await?;

            match peer_state {
                Some((peer_root_hash, peer_dag_heads)) if *peer_root_hash != [0; 32] => {
                    // Peer has state - decide strategy based on config
                    let peer_heads_count = peer_dag_heads.len();
                    let use_snapshot = strategy.should_use_snapshot(peer_heads_count);

                    info!(
                        %context_id,
                        %chosen_peer,
                        peer_root_hash = %peer_root_hash,
                        peer_heads_count,
                        use_snapshot,
                        %strategy,
                        "Peer has state, selecting sync strategy"
                    );

                    if use_snapshot {
                        // Also log which state sync strategy would be used if we had the protocols
                        let state_strategy = self.select_state_sync_strategy(
                            context_id,
                            false, // local has no data (fresh node)
                            0,
                            peer_heads_count * 10, // estimate remote entities
                            3,                     // default depth estimate
                            peer_heads_count,
                        );

                        info!(
                            %context_id,
                            fresh_node_strategy = %strategy,
                            state_sync_strategy = %state_strategy,
                            "Fresh node using snapshot sync (state strategy logged for reference)"
                        );

                        // Use snapshot sync for efficient bootstrap
                        // Note: request_snapshot_sync opens its own stream, existing stream
                        // will be closed when this function returns
                        match self.request_snapshot_sync(context_id, chosen_peer).await {
                            Ok(result) => {
                                // Record snapshot metrics
                                self.metrics
                                    .record_snapshot_records(result.applied_records as u64);

                                info!(
                                    %context_id,
                                    %chosen_peer,
                                    applied_records = result.applied_records,
                                    boundary_root_hash = %result.boundary_root_hash,
                                    dag_heads_count = result.dag_heads.len(),
                                    "Snapshot sync completed successfully"
                                );
                                return Ok(Some(SyncProtocol::SnapshotSync));
                            }
                            Err(e) => {
                                warn!(
                                    %context_id,
                                    %chosen_peer,
                                    error = %e,
                                    "Snapshot sync failed, will retry with another peer"
                                );
                                bail!("Snapshot sync failed: {}", e);
                            }
                        }
                    } else {
                        // Use delta sync - fetch deltas one by one from genesis
                        info!(
                            %context_id,
                            %chosen_peer,
                            peer_heads_count,
                            "Using delta sync for fresh node bootstrap (configured strategy)"
                        );

                        let result = self
                            .request_dag_heads_and_sync(
                                context_id,
                                chosen_peer,
                                our_identity,
                                stream,
                            )
                            .await?;

                        if matches!(result, SyncProtocol::None) {
                            bail!("Delta sync returned no protocol - peer may have no data");
                        }

                        return Ok(Some(result));
                    }
                }
                Some(_) => {
                    // Peer is also uninitialized, try next peer
                    info!(%context_id, %chosen_peer, "Peer also has no state, trying next peer");
                    bail!("Peer has no data for this context");
                }
                None => {
                    // Failed to query peer state
                    bail!("Failed to query peer state for context {}", context_id);
                }
            }
        }

        // Check if we have pending deltas (incomplete DAG)
        // Even if node has some state, it might be missing parent deltas
        if let Some(delta_store) = self.node_state.delta_stores.get(&context_id) {
            let missing_result = delta_store.get_missing_parents().await;

            // Note: Cascaded events from DB loads are handled in state_delta handler
            if !missing_result.cascaded_events.is_empty() {
                info!(
                    %context_id,
                    cascaded_count = missing_result.cascaded_events.len(),
                    "Cascaded deltas from DB load (handlers executed in state_delta path)"
                );
            }

            if !missing_result.missing_ids.is_empty() {
                warn!(
                    %context_id,
                    %chosen_peer,
                    missing_count = missing_result.missing_ids.len(),
                    "Node has incomplete DAG (pending deltas), requesting DAG heads to catch up"
                );

                // Request DAG heads just like uninitialized nodes
                let result = self
                    .request_dag_heads_and_sync(context_id, chosen_peer, our_identity, stream)
                    .await?;

                // If peer had no data, return error to try next peer
                if matches!(result, SyncProtocol::None) {
                    bail!("Peer has no data for this context");
                }

                return Ok(Some(result));
            }
        }

        // Compare our state with peer's state even if we think we're in sync.
        // The peer might have new heads we don't know about (e.g., if gossipsub messages were lost).
        let peer_state = self
            .query_peer_dag_state(context_id, chosen_peer, our_identity, stream)
            .await?;

        if let Some((peer_root_hash, peer_dag_heads)) = peer_state {
            if *context.root_hash != *peer_root_hash {
                info!(
                    %context_id,
                    %chosen_peer,
                    our_root_hash = %context.root_hash,
                    peer_root_hash = %peer_root_hash,
                    our_heads_count = context.dag_heads.len(),
                    peer_heads_count = peer_dag_heads.len(),
                    "Root hash mismatch with peer, triggering DAG catchup"
                );

                let our_heads_set: std::collections::HashSet<_> =
                    context.dag_heads.iter().collect();
                let missing_heads: Vec<_> = peer_dag_heads
                    .iter()
                    .filter(|h| !our_heads_set.contains(h))
                    .cloned()
                    .collect();

                if !missing_heads.is_empty() && !self.sync_config.force_state_sync {
                    info!(
                        %context_id,
                        %chosen_peer,
                        missing_count = missing_heads.len(),
                        "Peer has DAG heads we don't have, requesting them"
                    );

                    let result = self
                        .request_dag_heads_and_sync(context_id, chosen_peer, our_identity, stream)
                        .await?;

                    // If peer had no data or unexpected response, return error to try next peer
                    if matches!(result, SyncProtocol::None) {
                        bail!("Peer has no data or unexpected response for this context, will try next peer");
                    }

                    return Ok(Some(result));
                }

                // Force state sync mode OR same heads but different root hash
                if self.sync_config.force_state_sync && !missing_heads.is_empty() {
                    warn!(
                        %context_id,
                        %chosen_peer,
                        missing_heads_count = missing_heads.len(),
                        "BENCHMARK MODE: Bypassing DAG catchup, forcing state sync strategy"
                    );
                }

                {
                    // Same heads but different root hash - potential CRDT merge needed
                    // This can happen when concurrent writes create the same DAG structure
                    // but produce different Merkle tree states (e.g., different entry ordering)

                    // Select state sync strategy based on tree characteristics
                    // Note: We estimate entity count from DAG heads as a proxy
                    let local_entity_count = context.dag_heads.len() * 10; // Rough estimate
                    let remote_entity_count = peer_dag_heads.len() * 10;
                    let tree_depth = 3; // Default estimate, could query from storage
                    let child_count = context.dag_heads.len();

                    let strategy = self.select_state_sync_strategy(
                        context_id,
                        true, // local has data
                        local_entity_count,
                        remote_entity_count,
                        tree_depth,
                        child_count,
                    );

                    warn!(
                        %context_id,
                        %chosen_peer,
                        state_sync_strategy = %strategy,
                        "Same DAG heads but different root hash - state sync needed"
                    );

                    // Dispatch to the appropriate sync protocol based on selected strategy
                    let result = match strategy {
                        StateSyncStrategy::HashComparison => {
                            self.hash_comparison_sync(
                                context_id,
                                chosen_peer,
                                our_identity,
                                stream,
                                context.root_hash,
                                peer_root_hash,
                            )
                            .await?
                        }
                        StateSyncStrategy::BloomFilter {
                            false_positive_rate,
                        } => {
                            self.bloom_filter_sync(
                                context_id,
                                chosen_peer,
                                our_identity,
                                stream,
                                false_positive_rate,
                            )
                            .await?
                        }
                        StateSyncStrategy::SubtreePrefetch { max_depth } => {
                            self.subtree_prefetch_sync(
                                context_id,
                                chosen_peer,
                                our_identity,
                                stream,
                                context.root_hash,
                                peer_root_hash,
                                max_depth,
                            )
                            .await?
                        }
                        StateSyncStrategy::LevelWise { max_depth } => {
                            self.level_wise_sync(
                                context_id,
                                chosen_peer,
                                our_identity,
                                stream,
                                context.root_hash,
                                peer_root_hash,
                                max_depth,
                            )
                            .await?
                        }
                        // Adaptive already selected a concrete strategy, shouldn't reach here
                        StateSyncStrategy::Adaptive => {
                            self.hash_comparison_sync(
                                context_id,
                                chosen_peer,
                                our_identity,
                                stream,
                                context.root_hash,
                                peer_root_hash,
                            )
                            .await?
                        }
                        // Snapshot/CompressedSnapshot are blocked for initialized nodes
                        // by the safety check above, but handle defensively
                        StateSyncStrategy::Snapshot | StateSyncStrategy::CompressedSnapshot => {
                            warn!(
                                %context_id,
                                "Snapshot strategy should have been blocked for initialized node"
                            );
                            self.hash_comparison_sync(
                                context_id,
                                chosen_peer,
                                our_identity,
                                stream,
                                context.root_hash,
                                peer_root_hash,
                            )
                            .await?
                        }
                    };

                    // If peer had no data or unexpected response, return error to try next peer
                    if matches!(result, SyncProtocol::None) {
                        bail!("Peer has no data or unexpected response for this context, will try next peer");
                    }

                    return Ok(Some(result));
                }
            } else {
                debug!(
                    %context_id,
                    %chosen_peer,
                    root_hash = %context.root_hash,
                    "Root hash matches peer, node is truly in sync"
                );
            }
        }

        Ok(None)
    }

    /// Query peer for their DAG state (root_hash and dag_heads) without triggering full sync.
    ///
    /// Returns `Ok(Some((root_hash, dag_heads)))` if peer responded successfully,
    /// `Ok(None)` if peer had no valid response or no state, or `Err` on communication error.
    async fn query_peer_dag_state(
        &self,
        context_id: ContextId,
        chosen_peer: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<Option<(calimero_primitives::hash::Hash, Vec<[u8; DIGEST_SIZE]>)>> {
        let request_msg = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::DagHeadsRequest { context_id },
            next_nonce: rand::thread_rng().gen(),
        };

        self.send(stream, &request_msg, None).await?;

        let response = self.recv(stream, None).await?;

        match response {
            Some(StreamMessage::Message {
                payload:
                    MessagePayload::DagHeadsResponse {
                        dag_heads,
                        root_hash,
                    },
                ..
            }) => {
                debug!(
                    %context_id,
                    %chosen_peer,
                    heads_count = dag_heads.len(),
                    peer_root_hash = %root_hash,
                    "Received peer DAG state for comparison"
                );
                Ok(Some((root_hash, dag_heads)))
            }
            _ => {
                debug!(%context_id, %chosen_peer, "Failed to get peer DAG state for comparison");
                Ok(None)
            }
        }
    }

    async fn initiate_sync_inner(
        &self,
        context_id: ContextId,
        chosen_peer: PeerId,
    ) -> eyre::Result<SyncProtocol> {
        use super::dial_tracker::{DialResult, DialTracker};
        use super::metrics::{PhaseTimer, SyncPhaseTimings};

        // Initialize per-phase timing tracker
        let mut timings = SyncPhaseTimings::new();
        let sync_start = std::time::Instant::now();

        // =====================================================================
        // PHASE 1: Peer Selection & Stream Setup (includes dial)
        // =====================================================================
        let phase_timer = PhaseTimer::start();

        let mut context = self
            .context_client
            .sync_context_config(context_id, None)
            .await?;

        // Get application - if not found, we'll try to install it after blob sharing
        let mut application = self.node_client.get_application(&context.application_id)?;

        // Get blob_id and app config for later use
        let (blob_id, app_config_opt) = self.get_blob_info(&context_id, &application).await?;

        let identities = self
            .context_client
            .get_context_members(&context.id, Some(true));

        let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
            .await
            .transpose()?
        else {
            bail!("no owned identities found for context: {}", context.id);
        };

        // =====================================================================
        // DIAL PHASE: Instrumented stream opening
        // =====================================================================
        // Check if we believe we're already connected
        let was_connected = {
            let state = self.connection_state.read().unwrap();
            state.is_likely_connected(&chosen_peer)
        };

        let dial_start = std::time::Instant::now();
        let stream_result = self.network_client.open_stream(chosen_peer).await;
        let dial_ms = dial_start.elapsed().as_secs_f64() * 1000.0;

        let mut stream = match stream_result {
            Ok(s) => {
                // Heuristic: fast dial (<50ms) suggests connection reuse
                let reused = was_connected || dial_ms < 50.0;

                let mut dial_tracker = DialTracker::new(chosen_peer, was_connected, 1);
                dial_tracker.start_dial();
                dial_tracker.end_dial(DialResult::Success, reused);

                // Update connection state
                {
                    let mut state = self.connection_state.write().unwrap();
                    state.get_mut(chosen_peer).on_success(dial_ms);
                }

                // Record in pool stats
                let breakdown = dial_tracker.finish(&context_id.to_string());
                {
                    let mut stats = self.dial_pool_stats.write().unwrap();
                    stats.record(&breakdown);
                }

                s
            }
            Err(e) => {
                let mut dial_tracker = DialTracker::new(chosen_peer, was_connected, 1);
                dial_tracker.start_dial();
                dial_tracker.end_dial(DialResult::Error, false);
                let breakdown = dial_tracker.finish(&context_id.to_string());

                // Update connection state
                {
                    let mut state = self.connection_state.write().unwrap();
                    state.get_mut(chosen_peer).on_failure();
                }

                // Record in pool stats
                {
                    let mut stats = self.dial_pool_stats.write().unwrap();
                    stats.record(&breakdown);
                }

                return Err(e);
            }
        };

        timings.peer_selection_ms = phase_timer.stop();

        // =====================================================================
        // PHASE 2: Protocol Negotiation (Handshake)
        // =====================================================================
        let phase_timer = PhaseTimer::start();

        let handshake_response = self
            .initiate_sync_handshake(&context, our_identity, &mut stream)
            .await?;

        let negotiated_protocol = handshake_response.negotiated_protocol.clone();
        let peer_root_hash = handshake_response.root_hash;

        // Check if we need to sync at all (root hashes match)
        let needs_sync = context.root_hash != peer_root_hash;
        if !needs_sync {
            debug!(
                %context_id,
                "Root hashes match, no sync needed"
            );
        }

        timings.key_share_ms = phase_timer.stop(); // Reuse timing slot for handshake

        // =====================================================================
        // PHASE 3: Key Share
        // =====================================================================
        let phase_timer = PhaseTimer::start();

        self.initiate_key_share_process(&mut context, our_identity, &mut stream)
            .await?;

        timings.key_share_ms += phase_timer.stop(); // Add key share to handshake time

        // =====================================================================
        // PHASE 4: Blob Share (if needed)
        // =====================================================================
        if !self.node_client.has_blob(&blob_id)? {
            let phase_timer = PhaseTimer::start();

            // Get size from application config if we don't have application yet
            let size = self
                .get_application_size(&context_id, &application, &app_config_opt)
                .await?;

            self.initiate_blob_share_process(&context, our_identity, blob_id, size, &mut stream)
                .await?;

            // After blob sharing, try to install application if it doesn't exist
            if application.is_none() {
                self.install_bundle_after_blob_sharing(
                    &context_id,
                    &blob_id,
                    &app_config_opt,
                    &mut context,
                    &mut application,
                )
                .await?;
            }

            timings.data_transfer_ms += phase_timer.stop();
        }

        let Some(_application) = application else {
            bail!("application not found: {}", context.application_id);
        };

        // =====================================================================
        // PHASE 5: State Sync (using negotiated protocol)
        // =====================================================================
        let phase_timer = PhaseTimer::start();

        // Decide sync strategy based on negotiated protocol
        let result = if !needs_sync {
            // Root hashes already match - no sync needed
            timings.dag_compare_ms = phase_timer.stop();
            debug!(%context_id, "Root hashes match, skipping state sync");
            SyncProtocol::None
        } else {
            // Use negotiated protocol to decide sync approach
            use calimero_node_primitives::sync_protocol::SyncProtocolVersion;

            let sync_result = match &negotiated_protocol {
                Some(SyncProtocolVersion::SnapshotSync { .. }) => {
                    // Peer suggested snapshot sync - use it for large divergence
                    info!(%context_id, "Using negotiated SnapshotSync");
                    self.handle_dag_sync(
                        context_id,
                        &context,
                        chosen_peer,
                        our_identity,
                        &mut stream,
                    )
                    .await?
                }
                Some(SyncProtocolVersion::HybridSync { .. }) => {
                    // Hybrid sync - try hash-based tree comparison with CRDT merge
                    info!(%context_id, "Using negotiated HybridSync (hash-based tree comparison)");
                    self.handle_tree_sync_with_callback(
                        context_id,
                        &context,
                        chosen_peer,
                        our_identity,
                        &mut stream,
                    )
                    .await?
                }
                Some(SyncProtocolVersion::DeltaSync { .. }) | None => {
                    // Default to DAG-based delta sync
                    info!(%context_id, protocol=?negotiated_protocol, "Using DeltaSync (DAG-based)");
                    self.handle_dag_sync(
                        context_id,
                        &context,
                        chosen_peer,
                        our_identity,
                        &mut stream,
                    )
                    .await?
                }
            };

            timings.dag_compare_ms = phase_timer.stop();
            sync_result.unwrap_or_else(|| {
                debug!(%context_id, "No active sync protocol needed");
                SyncProtocol::None
            })
        };

        // =====================================================================
        // Log phase breakdown
        // =====================================================================
        timings.total_ms = sync_start.elapsed().as_secs_f64() * 1000.0;

        // Log detailed breakdown (searchable with SYNC_PHASE_BREAKDOWN)
        timings.log(
            &context_id.to_string(),
            &chosen_peer.to_string(),
            &format!("{:?}", result),
        );

        // Record to Prometheus
        self.metrics.record_phase_timings(&timings);

        Ok(result)
    }

    /// Request peer's DAG heads and sync all missing deltas
    pub(super) async fn request_dag_heads_and_sync(
        &self,
        context_id: ContextId,
        peer_id: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<SyncProtocol> {
        use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};

        // Send DAG heads request
        let request_msg = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::DagHeadsRequest { context_id },
            next_nonce: {
                use rand::Rng;
                rand::thread_rng().gen()
            },
        };

        self.send(stream, &request_msg, None).await?;

        // Receive response
        let response = self.recv(stream, None).await?;

        match response {
            Some(StreamMessage::Message {
                payload:
                    MessagePayload::DagHeadsResponse {
                        dag_heads,
                        root_hash,
                    },
                ..
            }) => {
                info!(
                    %context_id,
                    heads_count = dag_heads.len(),
                    peer_root_hash = %root_hash,
                    "Received DAG heads from peer, requesting deltas"
                );

                // Check if peer has state even without DAG heads
                if dag_heads.is_empty() && *root_hash != [0; 32] {
                    error!(
                        %context_id,
                        peer_root_hash = %root_hash,
                        "Peer has state but no DAG heads!"
                    );
                    bail!(
                        "Peer has state but no DAG heads (migration issue). \
                         Clear data directories on both nodes and recreate context."
                    );
                }

                if dag_heads.is_empty() {
                    info!(%context_id, "Peer also has no deltas and no state, will try next peer");
                    // Return None to signal caller to try next peer
                    return Ok(SyncProtocol::None);
                }

                // CRITICAL FIX: Fetch ALL DAG heads first, THEN request missing parents
                // This ensures we don't miss sibling heads that might be the missing parents

                // Get or create DeltaStore for this context (do this once before the loop)
                let (delta_store_ref, is_new_store) = {
                    let mut is_new = false;
                    let delta_store = self
                        .node_state
                        .delta_stores
                        .entry(context_id)
                        .or_insert_with(|| {
                            is_new = true;
                            crate::delta_store::DeltaStore::new(
                                [0u8; 32],
                                self.context_client.clone(),
                                context_id,
                                our_identity,
                            )
                        });

                    let delta_store_ref = delta_store.clone();
                    (delta_store_ref, is_new)
                };

                // Load persisted deltas from database on first access
                if is_new_store {
                    if let Err(e) = delta_store_ref.load_persisted_deltas().await {
                        warn!(
                            ?e,
                            %context_id,
                            "Failed to load persisted deltas, starting with empty DAG"
                        );
                    }
                }

                // Phase 1: Request and add ALL DAG heads
                for head_id in &dag_heads {
                    info!(
                        %context_id,
                        head_id = ?head_id,
                        "Requesting DAG head delta from peer"
                    );

                    let delta_request = StreamMessage::Init {
                        context_id,
                        party_id: our_identity,
                        payload: InitPayload::DeltaRequest {
                            context_id,
                            delta_id: *head_id,
                        },
                        next_nonce: {
                            use rand::Rng;
                            rand::thread_rng().gen()
                        },
                    };

                    self.send(stream, &delta_request, None).await?;

                    let delta_response = self.recv(stream, None).await?;

                    match delta_response {
                        Some(StreamMessage::Message {
                            payload: MessagePayload::DeltaResponse { delta },
                            ..
                        }) => {
                            // Deserialize and add to DAG
                            let storage_delta: calimero_storage::delta::CausalDelta =
                                borsh::from_slice(&delta)?;

                            let dag_delta = calimero_dag::CausalDelta::new(
                                storage_delta.id,
                                storage_delta.parents,
                                storage_delta.actions,
                                storage_delta.hlc,
                                storage_delta.expected_root_hash,
                            );

                            if let Err(e) = delta_store_ref.add_delta(dag_delta).await {
                                warn!(
                                    ?e,
                                    %context_id,
                                    head_id = ?head_id,
                                    "Failed to add DAG head delta"
                                );
                            } else {
                                info!(
                                    %context_id,
                                    head_id = ?head_id,
                                    "Successfully added DAG head delta"
                                );
                            }
                        }
                        Some(StreamMessage::Message {
                            payload:
                                MessagePayload::SnapshotError {
                                    error:
                                        calimero_node_primitives::sync::SnapshotError::SnapshotRequired,
                                },
                            ..
                        }) => {
                            info!(
                                %context_id,
                                head_id = ?head_id,
                                "Peer's delta history is pruned, falling back to snapshot sync"
                            );
                            // Fall back to snapshot sync
                            return self
                                .fallback_to_snapshot_sync(
                                    context_id,
                                    our_identity,
                                    peer_id,
                                    stream,
                                )
                                .await;
                        }
                        Some(StreamMessage::Message {
                            payload: MessagePayload::DeltaNotFound,
                            ..
                        }) => {
                            warn!(
                                %context_id,
                                head_id = ?head_id,
                                "Peer doesn't have requested DAG head delta"
                            );
                            // Continue trying other heads
                        }
                        _ => {
                            warn!(%context_id, head_id = ?head_id, "Unexpected response to delta request");
                        }
                    }
                }

                // Phase 2: Now check for missing parents and fetch them recursively
                let missing_result = delta_store_ref.get_missing_parents().await;

                // Note: Cascaded events from DB loads logged but not executed here (state_delta handler will catch them)
                if !missing_result.cascaded_events.is_empty() {
                    info!(
                        %context_id,
                        cascaded_count = missing_result.cascaded_events.len(),
                        "Cascaded deltas from DB load during DAG head sync"
                    );
                }

                if !missing_result.missing_ids.is_empty() {
                    info!(
                        %context_id,
                        missing_count = missing_result.missing_ids.len(),
                        "DAG heads have missing parents, requesting them recursively"
                    );

                    // Request missing parents (this uses recursive topological fetching)
                    if let Err(e) = self
                        .request_missing_deltas(
                            context_id,
                            missing_result.missing_ids,
                            peer_id,
                            delta_store_ref.clone(),
                            our_identity,
                        )
                        .await
                    {
                        warn!(
                            ?e,
                            %context_id,
                            "Failed to request missing parent deltas during DAG catchup"
                        );
                    }
                }

                // Return a non-None protocol to signal success (prevents trying next peer)
                Ok(SyncProtocol::DagCatchup)
            }
            _ => {
                warn!(%context_id, "Unexpected response to DAG heads request, trying next peer");
                Ok(SyncProtocol::None)
            }
        }
    }

    /// Fall back to full snapshot sync when delta sync is not possible.
    async fn fallback_to_snapshot_sync(
        &self,
        context_id: ContextId,
        our_identity: PublicKey,
        peer_id: PeerId,
        _stream: &mut Stream,
    ) -> eyre::Result<SyncProtocol> {
        info!(%context_id, %peer_id, "Initiating snapshot sync");

        let result = self.request_snapshot_sync(context_id, peer_id).await?;

        // Record snapshot metrics
        self.metrics
            .record_snapshot_records(result.applied_records as u64);

        info!(%context_id, records = result.applied_records, "Snapshot sync completed");

        // Fine-sync to catch any deltas since the snapshot boundary
        if !result.dag_heads.is_empty() {
            let mut stream = self.network_client.open_stream(peer_id).await?;
            if let Err(e) = self
                .fine_sync_from_boundary(context_id, peer_id, our_identity, &mut stream)
                .await
            {
                warn!(?e, %context_id, "Fine-sync failed, state may be slightly behind");
            }
        }

        Ok(SyncProtocol::SnapshotSync)
    }

    /// Fine-sync from snapshot boundary to catch up to latest state.
    async fn fine_sync_from_boundary(
        &self,
        context_id: ContextId,
        peer_id: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        let delta_store = self
            .node_state
            .delta_stores
            .entry(context_id)
            .or_insert_with(|| {
                crate::delta_store::DeltaStore::new(
                    [0u8; 32],
                    self.context_client.clone(),
                    context_id,
                    our_identity,
                )
            })
            .clone();

        let _ = delta_store.load_persisted_deltas().await;

        let request_msg = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::DagHeadsRequest { context_id },
            next_nonce: rand::random(),
        };
        self.send(stream, &request_msg, None).await?;

        let response = self.recv(stream, None).await?;

        if let Some(StreamMessage::Message {
            payload: MessagePayload::DagHeadsResponse { dag_heads, .. },
            ..
        }) = response
        {
            let mut missing = Vec::new();
            for head in &dag_heads {
                if !delta_store.has_delta(head).await {
                    missing.push(*head);
                }
            }

            if !missing.is_empty() {
                self.request_missing_deltas(
                    context_id,
                    missing,
                    peer_id,
                    delta_store,
                    our_identity,
                )
                .await?;
            }
        }

        Ok(())
    }

    pub async fn handle_opened_stream(&self, mut stream: Box<Stream>) {
        loop {
            match self.internal_handle_opened_stream(&mut stream).await {
                Ok(None) => break,
                Ok(Some(())) => {}
                Err(err) => {
                    error!(%err, "Failed to handle stream message");

                    if let Err(err) = self
                        .send(&mut stream, &StreamMessage::OpaqueError, None)
                        .await
                    {
                        error!(%err, "Failed to send error message");
                    }
                }
            }
        }
    }

    async fn internal_handle_opened_stream(&self, stream: &mut Stream) -> eyre::Result<Option<()>> {
        let Some(message) = self.recv(stream, None).await? else {
            return Ok(None);
        };

        let (context_id, their_identity, payload, nonce) = match message {
            StreamMessage::Init {
                context_id,
                party_id,
                payload,
                next_nonce,
                ..
            } => (context_id, party_id, payload, next_nonce),
            unexpected @ (StreamMessage::Message { .. } | StreamMessage::OpaqueError) => {
                bail!("expected initialization handshake, got {:?}", unexpected)
            }
        };

        let Some(context) = self.context_client.get_context(&context_id)? else {
            bail!("context not found: {}", context_id);
        };

        let mut _updated = None;

        if !self
            .context_client
            .has_member(&context_id, &their_identity)?
        {
            _updated = Some(
                self.context_client
                    .sync_context_config(context_id, None)
                    .await?,
            );

            if !self
                .context_client
                .has_member(&context_id, &their_identity)?
            {
                bail!(
                    "unknown context member {} in context {}",
                    their_identity,
                    context_id
                );
            }
        }

        // Note: Concurrent syncs are already prevented by SyncState tracking
        // in the start() loop. When sync starts, last_sync is set to None.
        // When complete, it's set to Some(now).

        let identities = self
            .context_client
            .get_context_members(&context.id, Some(true));

        let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
            .await
            .transpose()?
        else {
            bail!("no owned identities found for context: {}", context.id);
        };

        match payload {
            InitPayload::KeyShare => {
                self.handle_key_share_request(&context, our_identity, their_identity, stream, nonce)
                    .await?
            }
            InitPayload::BlobShare { blob_id } => {
                self.handle_blob_share_request(
                    &context,
                    our_identity,
                    their_identity,
                    blob_id,
                    stream,
                )
                .await?
            }
            // Old sync protocols removed - DAG uses gossipsub broadcast instead
            // Streams are only used for: KeyShare, BlobShare, DeltaRequest, DagHeadsRequest
            InitPayload::DeltaRequest {
                context_id: requested_context_id,
                delta_id,
            } => {
                // Handle delta request from peer
                self.handle_delta_request(requested_context_id, delta_id, stream)
                    .await?
            }
            InitPayload::DagHeadsRequest {
                context_id: requested_context_id,
            } => {
                // Handle DAG heads request from peer
                self.handle_dag_heads_request(requested_context_id, stream, nonce)
                    .await?
            }
            InitPayload::SnapshotBoundaryRequest {
                context_id: requested_context_id,
                requested_cutoff_timestamp,
            } => {
                // Handle snapshot boundary negotiation request from peer
                self.handle_snapshot_boundary_request(
                    requested_context_id,
                    requested_cutoff_timestamp,
                    stream,
                    nonce,
                )
                .await?
            }
            InitPayload::SnapshotStreamRequest {
                context_id: requested_context_id,
                boundary_root_hash,
                page_limit,
                byte_limit,
                resume_cursor,
            } => {
                // Handle snapshot stream request from peer
                self.handle_snapshot_stream_request(
                    requested_context_id,
                    boundary_root_hash,
                    page_limit,
                    byte_limit,
                    resume_cursor,
                    stream,
                    nonce,
                )
                .await?
            }
            InitPayload::SyncHandshake { handshake } => {
                // Handle sync handshake for protocol negotiation
                self.handle_sync_handshake(&context, handshake, stream, nonce)
                    .await?
            }
            InitPayload::TreeNodeRequest {
                context_id: requested_context_id,
                node_ids,
                include_children_depth,
            } => {
                // Handle tree node request for hash comparison sync
                self.handle_tree_node_request(
                    requested_context_id,
                    node_ids,
                    include_children_depth,
                    stream,
                    nonce,
                )
                .await?
            }
            InitPayload::BloomFilterRequest {
                context_id: requested_context_id,
                bloom_filter,
                false_positive_rate,
            } => {
                // Handle bloom filter request for efficient diff detection
                self.handle_bloom_filter_request(
                    requested_context_id,
                    bloom_filter,
                    false_positive_rate,
                    stream,
                    nonce,
                )
                .await?
            }
        };

        Ok(Some(()))
    }

    /// Handle incoming sync handshake for protocol negotiation.
    async fn handle_sync_handshake(
        &self,
        context: &calimero_primitives::context::Context,
        handshake: calimero_node_primitives::sync_protocol::SyncHandshake,
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        use calimero_node_primitives::sync::MessagePayload;
        use calimero_node_primitives::sync_protocol::{SyncCapabilities, SyncHandshakeResponse};

        info!(
            context_id = %context.id,
            peer_root_hash = %handshake.root_hash,
            peer_entity_count = handshake.entity_count,
            peer_dag_heads = handshake.dag_heads.len(),
            "Received sync handshake"
        );

        // Our capabilities
        let our_caps = SyncCapabilities::full();

        // Negotiate protocol
        let negotiated_protocol = our_caps.negotiate(&handshake.capabilities);

        if negotiated_protocol.is_none() {
            warn!(
                context_id = %context.id,
                "No common sync protocol with peer"
            );
        }

        // Build response
        let response = SyncHandshakeResponse {
            negotiated_protocol,
            capabilities: our_caps,
            root_hash: context.root_hash,
            dag_heads: context.dag_heads.clone(),
            entity_count: 0, // TODO: Get actual entity count from storage
        };

        let msg = StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::SyncHandshakeResponse { response },
            next_nonce: nonce,
        };

        self.send(stream, &msg, None).await?;

        Ok(())
    }

    /// Handle tree node request for hash comparison sync.
    ///
    /// For root requests (empty node_ids), returns a summary with all entity keys as children.
    /// For specific node requests, returns the entity data as leaf_data.
    async fn handle_tree_node_request(
        &self,
        context_id: ContextId,
        node_ids: Vec<[u8; 32]>,
        include_children_depth: u8,
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        use super::snapshot::get_entity_keys;
        use calimero_store::key::ContextState as ContextStateKey;

        info!(
            %context_id,
            node_count = node_ids.len(),
            include_children_depth,
            "Handling tree node request"
        );

        // Get context to access root hash
        let context = self
            .context_client
            .get_context(&context_id)?
            .ok_or_else(|| eyre::eyre!("Context not found"))?;

        let store_handle = self.context_client.datastore_handle();

        let nodes = if node_ids.is_empty() {
            // Root node request - return summary with all entity keys as children
            let entity_keys = get_entity_keys(&store_handle, context_id)?;

            info!(
                %context_id,
                entity_count = entity_keys.len(),
                "Returning root node with entity keys as children"
            );

            // Create children from entity keys
            // Each entity is treated as a leaf, so hash = entity key hash
            let children: Vec<TreeNodeChild> = entity_keys
                .iter()
                .map(|key| {
                    // Use key as both node_id and hash placeholder
                    // In a full Merkle tree, we'd compute proper hashes
                    TreeNodeChild {
                        node_id: *key,
                        hash: calimero_primitives::hash::Hash::from(*key),
                    }
                })
                .collect();

            vec![TreeNode {
                node_id: [0; 32], // Root
                hash: context.root_hash,
                leaf_data: None,
                children,
            }]
        } else {
            // Specific node requests - return entity data with metadata
            let mut result_nodes = Vec::new();

            for node_id in &node_ids {
                // Look up the entity data in storage
                let state_key = ContextStateKey::new(context_id, *node_id);

                let leaf_data = match store_handle.get(&state_key) {
                    Ok(Some(value)) => {
                        let value_bytes: Vec<u8> = value.as_ref().to_vec();

                        // Read entity metadata from Index
                        let id = calimero_storage::address::Id::from(*node_id);
                        let index_key_bytes = StorageKey::Index(id).to_bytes();
                        let index_state_key = ContextStateKey::new(context_id, index_key_bytes);

                        let metadata = match store_handle.get(&index_state_key) {
                            Ok(Some(index_value)) => {
                                match borsh::from_slice::<EntityIndex>(index_value.as_ref()) {
                                    Ok(index) => index.metadata.clone(),
                                    Err(e) => {
                                        warn!(
                                            %context_id,
                                            ?node_id,
                                            error = %e,
                                            "Failed to deserialize EntityIndex, using default metadata"
                                        );
                                        // Default to LwwRegister if we can't read metadata
                                        Metadata::new(0, 0)
                                    }
                                }
                            }
                            _ => {
                                // No index found, use default LwwRegister metadata
                                debug!(
                                    %context_id,
                                    ?node_id,
                                    "No EntityIndex found, using default LwwRegister metadata"
                                );
                                Metadata::new(0, 0)
                            }
                        };

                        // Create TreeLeafData with key, value, and metadata
                        Some(TreeLeafData {
                            key: *node_id,
                            value: value_bytes,
                            metadata,
                        })
                    }
                    _ => None,
                };

                result_nodes.push(TreeNode {
                    node_id: *node_id,
                    hash: calimero_primitives::hash::Hash::from(*node_id),
                    leaf_data,
                    children: vec![], // Entities are leaves, no children
                });
            }

            result_nodes
        };

        let msg = StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::TreeNodeResponse { nodes },
            next_nonce: nonce,
        };

        self.send(stream, &msg, None).await?;

        Ok(())
    }

    /// Handle bloom filter request for efficient diff detection.
    ///
    /// Checks our ENTITIES against the remote's bloom filter and
    /// returns any entities they're missing.
    async fn handle_bloom_filter_request(
        &self,
        context_id: ContextId,
        bloom_filter: Vec<u8>,
        false_positive_rate: f32,
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        use super::snapshot::get_entities_not_in_bloom;
        use calimero_storage::entities::Metadata;
        use calimero_storage::index::EntityIndex;
        use calimero_storage::store::Key as StorageKey;
        use calimero_store::key::ContextState as ContextStateKey;

        info!(
            %context_id,
            filter_size = bloom_filter.len(),
            false_positive_rate,
            "Handling ENTITY-based bloom filter request"
        );

        // Parse bloom filter metadata
        if bloom_filter.len() < 5 {
            warn!(%context_id, "Invalid bloom filter: too small");
            let msg = StreamMessage::Message {
                sequence_id: 0,
                payload: MessagePayload::BloomFilterResponse {
                    missing_entities: vec![],
                    matched_count: 0,
                },
                next_nonce: nonce,
            };
            self.send(stream, &msg, None).await?;
            return Ok(());
        }

        // Get storage handle via context_client
        let store_handle = self.context_client.datastore_handle();

        // Get entities NOT in the remote's bloom filter
        let missing_entries = get_entities_not_in_bloom(&store_handle, context_id, &bloom_filter)?;

        // Get total entity count for matched calculation
        let total_entities = {
            use super::snapshot::get_entity_keys;
            get_entity_keys(&store_handle, context_id)?.len() as u32
        };
        let missing_count = missing_entries.len() as u32;
        let matched = total_entities.saturating_sub(missing_count);

        // Build TreeLeafData for each entity WITH metadata
        let mut missing_entities_with_metadata: Vec<TreeLeafData> = Vec::new();
        for (key, value) in &missing_entries {
            // Read entity metadata from Index (same pattern as handle_tree_node_request)
            let id = calimero_storage::address::Id::from(*key);
            let index_key_bytes = StorageKey::Index(id).to_bytes();
            let index_state_key = ContextStateKey::new(context_id, index_key_bytes);

            let metadata = match store_handle.get(&index_state_key) {
                Ok(Some(index_value)) => {
                    match borsh::from_slice::<EntityIndex>(index_value.as_ref()) {
                        Ok(index) => index.metadata.clone(),
                        Err(e) => {
                            warn!(
                                %context_id,
                                ?key,
                                error = %e,
                                "Failed to deserialize EntityIndex for bloom filter, using default"
                            );
                            Metadata::new(0, 0)
                        }
                    }
                }
                _ => {
                    debug!(
                        %context_id,
                        ?key,
                        "No EntityIndex found for bloom filter entity, using default"
                    );
                    Metadata::new(0, 0)
                }
            };

            missing_entities_with_metadata.push(TreeLeafData {
                key: *key,
                value: value.clone(),
                metadata,
            });
        }

        info!(
            %context_id,
            missing_count,
            matched,
            "Bloom filter check complete, returning missing ENTITIES with metadata"
        );

        let msg = StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::BloomFilterResponse {
                missing_entities: missing_entities_with_metadata,
                matched_count: matched,
            },
            next_nonce: nonce,
        };

        self.send(stream, &msg, None).await?;

        Ok(())
    }
}
