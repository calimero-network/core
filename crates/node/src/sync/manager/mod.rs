//! Sync manager and orchestration.
//!
//! **Purpose**: Coordinates periodic syncs, selects peers, and delegates to protocols.
//! **Strategy**: Try delta sync first, fallback to state sync on failure.
use calimero_context::group_store::{
    CapabilitiesRepository, GroupKeyring, MembershipRepository, MetaRepository, NamespaceRepository,
};
use std::sync::Arc;

use calimero_context_client::client::ContextClient;
use calimero_crypto::{Nonce, SharedKey};
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::client::{NamespaceJoinParams, NodeClient, OpenSubgroupJoinParams};
use calimero_node_primitives::join_bundle::JoinBundle;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::common::DIGEST_SIZE;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use eyre::bail;
use eyre::WrapErr;
use futures_util::stream::{self};
use futures_util::StreamExt;
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;
use rand::seq::SliceRandom;
use rand::Rng;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{self, Instant};
use tracing::{debug, error, info, warn};

use crate::sync_session_bridge::{SyncSessionResult, SyncSessionSender};
use crate::utils::choose_stream;

use super::config::SyncConfig;
// `SyncState` + the `TrackingSyncProtocol` alias moved to `super::session`
// (Phase 3 of #2313). HashComparison + LevelWise initiator dispatch moved
// to `super::protocol_selector` (Phase 4). The run-loop + select! body
// moved to `super::driver` (Phase 5). `SyncProtocol` from primitives is
// still referenced here for protocol-selection types.
use calimero_node_primitives::sync::{
    build_handshake_from_raw, estimate_entity_count, estimate_max_depth, select_protocol,
    SyncHandshake, SyncProtocol,
};

/// Typed marker returned by [`SyncManager::recv`] when the responder
/// indicates the context is not materialised locally on the receiving
/// side (#2422 Option 4 — see `StreamMessage::NotMaterialized` doc).
///
/// Caught by `apply_session_result` and treated as benign:
/// - no `state.on_failure()` call (failure_count stays put)
/// - no exponential backoff (`backoff_delay` stays at last value)
/// - debug-only log (not warn)
///
/// The initiator simply drops this peer for this round and continues.
/// On the next sync tick the peer-selection filter
/// (`peers.rs::discover_mesh_peers_with_namespace_fallback`) should
/// have stopped picking this peer altogether, but the error stays as
/// a safety net for races (peer in flight of materialising, etc.).
#[derive(Debug, Clone, Copy)]
pub struct PeerNotMaterialized;

impl std::fmt::Display for PeerNotMaterialized {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("peer has not materialised this context locally")
    }
}

impl std::error::Error for PeerNotMaterialized {}

/// Typed marker returned by peer discovery when no peer is currently
/// available to sync a context with (the context-topic mesh is empty and
/// the namespace fallback found no follower).
///
/// Caught by `apply_session_result` and treated as benign — the same way
/// as [`PeerNotMaterialized`]:
/// - no `state.on_failure()` (failure_count stays put)
/// - no exponential backoff
/// - debug-only log (not warn)
///
/// "No peer right now" is a transient connectivity condition, not a sync
/// failure: counting it would inflate `failure_count` (which the dispatch
/// backoff keys on) and spam a misleading "applying exponential backoff"
/// warn while the node is simply waiting for a co-member to (re)appear —
/// exactly the post-restart window. The periodic tick keeps retrying;
/// once a peer shows up the next attempt proceeds normally.
#[derive(Debug, Clone, Copy)]
pub struct NoPeersAvailable {
    pub context_id: ContextId,
}

impl std::fmt::Display for NoPeersAvailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "No peers to sync with for context {}", self.context_id)
    }
}

impl std::error::Error for NoPeersAvailable {}

/// Network synchronization manager.
///
/// Orchestrates sync protocols: full resync, delta sync, state sync.
pub struct SyncManager {
    pub(crate) sync_config: SyncConfig,

    pub(super) node_client: NodeClient,
    pub(super) context_client: ContextClient,
    /// Concrete network client. Kept on the manager for external
    /// callers (e.g. handlers/network_event/*.rs) that need the full
    /// `NetworkClient` surface (`publish`, specialized-node-invite
    /// helpers, etc.) — sync itself only ever uses `open_stream` and
    /// `mesh_peers`, both of which are mediated by the
    /// [`SyncNetwork`] trait field below.
    pub(crate) network_client: NetworkClient,
    /// Sync's network surface, trait-mediated so tests can inject a
    /// `MockSyncNetwork` without spinning up Actix + libp2p. In
    /// production this is always `Arc::new(network_client.clone())`
    /// — the two fields hold the same underlying handle. See
    /// [`crate::sync::network::SyncNetwork`] for the contract.
    ///
    /// This split is the minimum-viable mockability: external callers
    /// keep talking to the concrete `NetworkClient` (no ripple), while
    /// sync's own methods read through the trait (mockable).
    pub(crate) sync_network: Arc<dyn super::network::SyncNetwork>,
    /// Sync's view of the node's mutable runtime state. Trait-mediated
    /// (`Arc<dyn SyncStateAccess>`) so tests can substitute a recording
    /// fake without spinning up a full `NodeManager`. In production
    /// this is always `Arc::new(node_state)` — the same `NodeState`
    /// the rest of the node holds.
    pub(super) state_access: Arc<dyn super::state_access::SyncStateAccess>,
    /// Concrete `NodeState` kept solely to hand off to the cross-actor
    /// `crate::handlers::state_delta::replay_buffered_delta` call,
    /// which uses a richer surface than `SyncStateAccess` exposes
    /// (governance-pending drain, delta-buffer operations, etc.). The
    /// rest of sync goes through `state_access`. Folding this last
    /// dependency requires either expanding `SyncStateAccess` to
    /// match `replay_buffered_delta`'s needs or restructuring the
    /// cross-actor call — both deferred per the spike that landed
    /// this trait.
    pub(super) node_state: crate::NodeState,

    pub(super) ctx_sync_rx: Option<mpsc::Receiver<(Option<ContextId>, Option<PeerId>)>>,
    pub(super) ns_sync_rx: Option<mpsc::Receiver<[u8; 32]>>,
    pub(super) ns_join_rx: Option<
        mpsc::Receiver<(
            NamespaceJoinParams,
            oneshot::Sender<eyre::Result<JoinBundle>>,
        )>,
    >,
    pub(super) open_subgroup_join_rx: Option<
        mpsc::Receiver<(
            OpenSubgroupJoinParams,
            oneshot::Sender<eyre::Result<Vec<u8>>>,
        )>,
    >,

    /// Dispatch handle for the dedicated `SyncSessionActor` (#2316).
    /// Set via [`SyncManager::set_session_handles`] after the actor is
    /// started; `None` on freshly-cloned instances (which never run
    /// the `start` loop) and on the original until wiring completes.
    pub(super) session_tx: Option<SyncSessionSender>,
    /// Channel the `SyncSessionActor` writes initiator results into so
    /// `start` can update per-context tracking state. Consumed once by
    /// `start`; `None` on clones.
    pub(super) session_result_rx: Option<mpsc::UnboundedReceiver<SyncSessionResult>>,

    /// Sync-protocol metrics collector. Installed by `run.rs::start` via
    /// [`SyncManager::set_metrics`] after the [`crate::sync::PrometheusSyncMetrics`]
    /// instance is registered against the global registry. `None` means
    /// recording sites use [`crate::sync::no_op_metrics`] as a silent
    /// fallback — never a panic and never a runtime cost beyond a vtable
    /// no-op.
    ///
    /// `dyn SyncMetricsCollector` does not implement `Debug`, so we
    /// hand-write a `Debug` impl on `SyncManager` (below) that prints
    /// only the presence/absence of this field — the inner vtable is
    /// opaque anyway.
    pub(crate) metrics: Option<Arc<dyn super::metrics::SyncMetricsCollector>>,

    /// Reconcile-after-divergence orchestrator. Owns the orchestration
    /// for [`Self::reconcile_after_divergence`]; that method is a thin
    /// forwarder. See `sync::reconciler`.
    pub(super) reconciler: super::reconciler::Reconciler,

    /// Protocol-dispatch for the initiator side of a sync session.
    /// Called from `handle_dag_sync` after `select_protocol` has
    /// chosen the protocol to run. See `sync::protocol_selector`.
    pub(super) protocol_selector: super::protocol_selector::ProtocolSelector,
}

impl std::fmt::Debug for SyncManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncManager")
            .field("sync_config", &self.sync_config)
            .field("metrics_installed", &self.metrics.is_some())
            .finish_non_exhaustive()
    }
}

impl Clone for SyncManager {
    fn clone(&self) -> Self {
        Self {
            sync_config: self.sync_config,
            node_client: self.node_client.clone(),
            context_client: self.context_client.clone(),
            network_client: self.network_client.clone(),
            sync_network: Arc::clone(&self.sync_network),
            state_access: Arc::clone(&self.state_access),
            node_state: self.node_state.clone(),
            ctx_sync_rx: None,
            ns_sync_rx: None,
            ns_join_rx: None,
            open_subgroup_join_rx: None,
            // Cloned `SyncManager`s never drive the `start` loop, so
            // they don't need a session-dispatch handle or a results
            // receiver. The bridge holds its own clone of the
            // SyncManager for issuing sessions.
            session_tx: None,
            session_result_rx: None,
            // Clones share the same metrics handle — Arc keeps the
            // recording surface unified across the original (which runs
            // `start`) and every responder/initiator clone.
            metrics: self.metrics.clone(),
            // Reconciler holds Arcs internally, so its `Clone` is
            // cheap and clones share the same state_access/sync_network
            // surfaces as the parent.
            reconciler: self.reconciler.clone(),
            // ProtocolSelector holds an `Arc<dyn SyncNetwork>` + a
            // `ContextClient`; cloning is cheap and shares the same
            // surfaces as the parent.
            protocol_selector: self.protocol_selector.clone(),
        }
    }
}

// Run-loop session-tracking moved to `crate::sync::session::SessionTracker`
// as Phase 3 of #2313. The free-fn predicates that used to live here
// (`dispatch_recently_attempted`, `session_dispatch_wedged`) and the
// nested `apply_session_result` helper now live alongside that struct.

impl SyncManager {
    pub(crate) fn new(
        sync_config: SyncConfig,
        node_client: NodeClient,
        context_client: ContextClient,
        network_client: NetworkClient,
        node_state: crate::NodeState,
        ctx_sync_rx: mpsc::Receiver<(Option<ContextId>, Option<PeerId>)>,
        ns_sync_rx: mpsc::Receiver<[u8; 32]>,
        ns_join_rx: mpsc::Receiver<(
            NamespaceJoinParams,
            oneshot::Sender<eyre::Result<JoinBundle>>,
        )>,
        open_subgroup_join_rx: mpsc::Receiver<(
            OpenSubgroupJoinParams,
            oneshot::Sender<eyre::Result<Vec<u8>>>,
        )>,
    ) -> Self {
        let sync_network: Arc<dyn super::network::SyncNetwork> = Arc::new(network_client.clone());
        // Wrap the concrete `NodeState` once here. The trait field is
        // sync's primary state surface; the concrete `node_state`
        // field is retained ONLY for the cross-actor
        // `replay_buffered_delta` handoff (see field doc).
        let state_access: Arc<dyn super::state_access::SyncStateAccess> =
            Arc::new(node_state.clone());
        let reconciler = super::reconciler::Reconciler::new(
            Arc::clone(&state_access),
            Arc::clone(&sync_network),
            context_client.clone(),
        );
        let protocol_selector =
            super::protocol_selector::ProtocolSelector::new(context_client.clone());
        Self {
            sync_config,
            node_client,
            context_client,
            network_client,
            sync_network,
            state_access,
            node_state,
            ctx_sync_rx: Some(ctx_sync_rx),
            ns_sync_rx: Some(ns_sync_rx),
            ns_join_rx: Some(ns_join_rx),
            open_subgroup_join_rx: Some(open_subgroup_join_rx),
            session_tx: None,
            session_result_rx: None,
            metrics: None,
            reconciler,
            protocol_selector,
        }
    }

    /// Test-only override of the sync network surface.
    ///
    /// Production code never calls this — the constructor wires
    /// `sync_network` from the concrete `NetworkClient` automatically.
    /// Tests use this to swap in a `MockSyncNetwork` after construction.
    ///
    /// Also rebuilds the [`super::reconciler::Reconciler`] and
    /// [`super::protocol_selector::ProtocolSelector`] fields so they
    /// observe the swapped network too. Without this, those components
    /// keep the construction-time `Arc` and their network calls on the
    /// reconcile / protocol-dispatch paths bypass the mock.
    #[cfg(test)]
    pub(crate) fn set_sync_network(&mut self, sync_network: Arc<dyn super::network::SyncNetwork>) {
        self.sync_network = Arc::clone(&sync_network);
        self.reconciler = super::reconciler::Reconciler::new(
            Arc::clone(&self.state_access),
            sync_network,
            self.context_client.clone(),
        );
        // `ProtocolSelector` doesn't hold a `sync_network` — the
        // dispatch trait's `open_stream` routes through `SyncManager`
        // (which now points at the swapped mock via `self.sync_network`).
        self.protocol_selector =
            super::protocol_selector::ProtocolSelector::new(self.context_client.clone());
    }

    /// Wire the `SyncSessionActor` handles onto the original
    /// `SyncManager` instance after the actor is started in `run.rs`.
    /// Must be called before [`SyncManager::start`]. No-op on cloned
    /// instances (those never run the `start` loop).
    pub(crate) fn set_session_handles(
        &mut self,
        session_tx: SyncSessionSender,
        session_result_rx: mpsc::UnboundedReceiver<SyncSessionResult>,
    ) {
        self.session_tx = Some(session_tx);
        self.session_result_rx = Some(session_result_rx);
    }

    /// Install the sync-protocol metrics collector. Must be called before
    /// any clones are taken; recording sites resolve `self.metrics` via
    /// [`SyncManager::metrics`] (which falls back to a no-op collector if
    /// this hasn't been called).
    pub(crate) fn set_metrics(&mut self, metrics: Arc<dyn super::metrics::SyncMetricsCollector>) {
        self.metrics = Some(metrics);
    }

    /// Resolve the metrics collector. Returns a static no-op handle when
    /// no collector was installed so call sites never have to branch on
    /// `Option` — `self.metrics().record_*()` is always valid.
    pub(crate) fn metrics(&self) -> &dyn super::metrics::SyncMetricsCollector {
        // The no-op fallback lives in a static OnceLock so it isn't
        // allocated per call. `NoOpMetrics` is a unit struct with
        // `Default`, so the init closure is `default()`.
        static NOOP: std::sync::OnceLock<super::metrics::NoOpMetrics> = std::sync::OnceLock::new();
        match self.metrics.as_deref() {
            Some(m) => m,
            None => NOOP.get_or_init(super::metrics::NoOpMetrics::default),
        }
    }

    /// Build `SyncHandshake` from local context state for protocol negotiation.
    ///
    /// Queries the real entity count and tree depth from the Merkle tree Index
    /// via the storage bridge. Falls back to estimation from DAG heads if the
    /// Index is not accessible (e.g., after snapshot sync with format mismatch).
    ///
    /// # Arguments
    ///
    /// * `context` - The context to build a handshake for.
    ///
    /// # Returns
    ///
    /// A `SyncHandshake` containing the context's current state summary.
    fn build_local_handshake(
        &self,
        context: &calimero_primitives::context::Context,
    ) -> SyncHandshake {
        let root_hash = *context.root_hash;
        let dag_heads = context.dag_heads.clone();

        // Try to get real entity count and depth from the Merkle tree Index.
        // This gives accurate protocol selection instead of guessing from dag_heads.
        let (entity_count, max_depth) = self.query_tree_stats(&context.id).unwrap_or_else(|| {
            // Fallback: estimate from dag_heads if Index is unavailable
            let count = estimate_entity_count(root_hash, dag_heads.len());
            let depth = estimate_max_depth(count);
            (count, depth)
        });

        build_handshake_from_raw(root_hash, entity_count, max_depth, dag_heads)
    }

    /// Query real entity count and tree depth from the Merkle tree Index.
    ///
    /// Returns `Some((entity_count, max_depth))` on success, `None` if the
    /// Index is unavailable (e.g., fresh node or deserialization mismatch).
    fn query_tree_stats(&self, context_id: &ContextId) -> Option<(u64, u32)> {
        use calimero_node_primitives::sync::create_runtime_env;
        use calimero_storage::address::Id;
        use calimero_storage::env::with_runtime_env;
        use calimero_storage::index::Index;
        use calimero_storage::store::MainStorage;

        let store = self.context_client.datastore_handle().into_inner();
        // SAFETY: identity is unused for read-only Index queries via RuntimeEnv
        let identity = calimero_primitives::identity::PublicKey::from([0u8; 32]);
        let env = create_runtime_env(&store, *context_id, identity);

        let root_id = Id::new(*context_id.as_ref());

        with_runtime_env(env, || {
            // Check if root Index exists
            let root_index = Index::<MainStorage>::get_index(root_id).ok().flatten()?;

            // Count children (leaf entities) under root.
            // Minimum 1 when root exists (consistent with fallback estimation).
            let children = root_index.children().unwrap_or_default();
            let entity_count = (children.len() as u64).max(1);

            // Depth: 1 when root has data (consistent with fallback).
            // For deeper trees, we'd need recursive traversal — tracked in #2054.
            let max_depth = 1;

            Some((entity_count, max_depth))
        })
    }

    /// Build `SyncHandshake` from peer state for protocol negotiation.
    ///
    /// Uses shared estimation functions from `calimero_node_primitives::sync::state_machine`
    /// to ensure consistent behavior between production (`SyncManager`) and simulation (`SimNode`).
    fn build_remote_handshake(
        peer_root_hash: calimero_primitives::hash::Hash,
        peer_dag_heads: &[[u8; DIGEST_SIZE]],
    ) -> SyncHandshake {
        let root_hash = *peer_root_hash;

        // Use shared estimation functions for consistency with simulation
        let entity_count = estimate_entity_count(root_hash, peer_dag_heads.len());
        let max_depth = estimate_max_depth(entity_count);

        build_handshake_from_raw(root_hash, entity_count, max_depth, peer_dag_heads.to_vec())
    }

    /// Run the sync-manager actor loop until the input channels close.
    ///
    /// Thin shell after Phase 5 of #2313: takes the channel handles
    /// off `self`, constructs a `SyncDriver` with the per-context
    /// `SessionTracker`, and delegates the actor loop to
    /// `SyncDriver::run`. The driver borrows `&self` for the
    /// cross-actor dispatch callbacks (namespace sync, namespace
    /// join, open-subgroup join) via the
    /// [`super::driver::SyncDriverDispatch`] trait that `SyncManager`
    /// implements.
    pub async fn start(mut self) {
        let Some(ctx_sync_rx) = self.ctx_sync_rx.take() else {
            error!("SyncManager can only be run once");
            return;
        };
        let ns_sync_rx = self.ns_sync_rx.take().unwrap_or_else(|| {
            let (_tx, rx) = mpsc::channel(1);
            rx
        });
        let ns_join_rx = self.ns_join_rx.take().unwrap_or_else(|| {
            let (_tx, rx) = mpsc::channel(1);
            rx
        });
        let open_subgroup_join_rx = self.open_subgroup_join_rx.take().unwrap_or_else(|| {
            let (_tx, rx) = mpsc::channel(1);
            rx
        });
        let Some(session_tx) = self.session_tx.clone() else {
            error!("SyncManager started without a SyncSessionActor handle (#2316)");
            return;
        };
        let Some(session_result_rx) = self.session_result_rx.take() else {
            error!("SyncManager started without a SyncSessionActor result channel (#2316)");
            return;
        };

        let tracker = super::session::SessionTracker::new(
            self.sync_config.session_deadline,
            self.sync_config.interval,
            self.node_state.sync_status_handle(),
        );

        let driver = super::driver::SyncDriver::new(
            tracker,
            self.context_client.clone(),
            ctx_sync_rx,
            ns_sync_rx,
            ns_join_rx,
            open_subgroup_join_rx,
            session_tx,
            session_result_rx,
            self.sync_config.frequency,
            self.sync_config.interval,
        );

        driver.run(&self).await;
    }

    pub(crate) async fn perform_interval_sync(
        &self,
        context_id: ContextId,
        peer_id: Option<PeerId>,
    ) -> eyre::Result<(PeerId, SyncProtocol)> {
        // #2625: release any state deltas parked in the governance-pending
        // buffer for this context before the regular context sync runs. The
        // cross-DAG check buffers a state delta as `Unknown` when the
        // namespace governance op its signed position references is missing
        // locally; the buffer normally drains when that op arrives via
        // gossip. But in a one-directional divergence the authoring peer
        // already applied the op and never rebroadcasts it, and our own
        // governance DAG never registers it as a missing parent (nothing
        // local references it except the buffered delta) — so neither the
        // gossip-apply drain nor `resolve_namespace_pending` (which gates on
        // `namespace_has_pending`) ever fires, and the context root stays
        // split-brain. Pulling the namespace governance DAG here lands the
        // op and triggers the drain. Cheap when nothing is buffered.
        self.backfill_governance_for_pending_deltas(context_id)
            .await;

        if let Some(peer_id) = peer_id {
            return self.initiate_sync(context_id, peer_id).await;
        }

        // Check if we're uninitialized before peer discovery so we can use
        // a longer mesh wait window for bootstrap scenarios.
        let context = self
            .context_client
            .get_context(&context_id)?
            .ok_or_else(|| eyre::eyre!("Context not found: {}", context_id))?;

        let is_uninitialized = *context.root_hash == [0; 32];

        // Retry peer discovery if mesh is still forming.
        // Uninitialized nodes need a longer wait window (10s vs 1.5s) to avoid
        // getting stuck before first snapshot sync. Gossipsub mesh takes 5-10
        // heartbeats (~5-10s) to add a new subscriber after topic subscription.
        let (max_retries, retry_delay_ms) = if is_uninitialized {
            (
                super::config::DEFAULT_MESH_RETRIES_UNINITIALIZED,
                super::config::DEFAULT_MESH_RETRY_DELAY_MS_UNINITIALIZED,
            )
        } else {
            (
                super::config::DEFAULT_MESH_RETRIES_INITIALIZED,
                super::config::DEFAULT_MESH_RETRY_DELAY_MS_INITIALIZED,
            )
        };

        // Resolve the namespace-root topic ONCE here for the
        // namespace-fallback closure passed to the discovery helper.
        // `get_context_group_id` returns the IMMEDIATE owning group
        // (which for a subgroup-owned context is the subgroup id, not
        // the namespace root). Only namespace roots have `ns/<id>`
        // topics subscribed (see `NodeClient::subscribe_namespace`),
        // so we walk up the parent chain to find the root before
        // computing the fallback topic. Without this walk, contexts
        // owned by subgroups always get 0 peers from the fallback and
        // sync fails during the 5-10s cold-start window.
        // `resolve_namespace` on a root group is a no-op (returns the
        // same id).
        let context_client = self.context_client.clone();
        // `move` captures `context_id` by copy (`ContextId` is `[u8; 32]`),
        // so `context_id` remains usable in the call below and in the
        // `info!` log emitted after the discovery returns.
        let resolve_namespace_topic = move || {
            let group_id = context_client.get_context_group_id(&context_id).ok()??;
            let store = context_client.datastore_handle().into_inner();
            let ns_id_bytes = NamespaceRepository::new(&store)
                .resolve(&calimero_context_config::types::ContextGroupId::from(
                    group_id,
                ))
                .map(|id| id.to_bytes())
                .unwrap_or_else(|err| {
                    // Errors here are rare and always indicate something
                    // worth investigating: store I/O failure or a
                    // circular parent chain exceeding
                    // MAX_NAMESPACE_DEPTH. Surface them before falling
                    // back so this debugging-focused code path doesn't
                    // hide real data-integrity bugs. Falling back to the
                    // immediate owning group preserves pre-extraction
                    // behaviour rather than aborting the whole sync
                    // attempt.
                    warn!(
                        %context_id,
                        %err,
                        "failed to resolve namespace root for fallback topic; \
                         using immediate group id as best-effort"
                    );
                    group_id
                });
            Some(TopicHash::from_raw(format!(
                "ns/{}",
                hex::encode(ns_id_bytes)
            )))
        };

        let outcome = super::peers::discover_mesh_peers_with_namespace_fallback(
            &*self.sync_network,
            context_id,
            max_retries,
            std::time::Duration::from_millis(retry_delay_ms),
            resolve_namespace_topic,
        )
        .await?;
        let peers = outcome.peers;
        let final_attempt = outcome.attempts;
        let mesh_elapsed = outcome.elapsed;

        info!(
            %context_id,
            peer_count = peers.len(),
            attempts = final_attempt,
            ?mesh_elapsed,
            is_uninitialized,
            source = ?outcome.source,
            "Mesh peer discovery succeeded"
        );

        if is_uninitialized {
            // When uninitialized, we need to bootstrap from a peer that HAS data
            // Trying random peers can result in querying other uninitialized nodes
            info!(
                %context_id,
                peer_count = peers.len(),
                "Node is uninitialized, selecting peer with state for bootstrapping"
            );

            // Try to find a peer with actual state
            match self.find_peer_with_state(context_id, &peers).await {
                Ok(peer_id) => {
                    info!(%context_id, %peer_id, "Found peer with state, syncing from them");
                    return self.initiate_sync(context_id, peer_id).await;
                }
                Err(e) => {
                    warn!(%context_id, error = %e, "Failed to find peer with state, falling back to random selection");
                    // Fall through to random selection
                }
            }
        }

        // Normal sync: try peers serially. Parallelising `initiate_sync` for
        // the same context is unsafe — the sync protocol mutates per-context
        // state (sync-in-progress marker at snapshot.rs:581, sync sessions at
        // state.rs:235, snapshot-page cleanup in `request_and_apply_snapshot_pages`
        // which documents "assumes no concurrent writes") and futures cancelled
        // mid-flight can leak a sync session into the DashMap, causing
        // `should_buffer_delta` to return true permanently. Tail-latency
        // benefit is still obtained from the parallel probe above, which
        // narrows this loop to "try a known-good peer first".
        //
        // Peer order: random shuffle, then stable-partition so peers we
        // have observed signing applied messages with an
        // Owner/Admin/ReadOnlyTee identity come first. Anchors are the
        // peers whose canonical view is authoritative — targeting them
        // first reduces the chance of pulling from a peer that's
        // behind or divergent. Plain members still get tried if all
        // anchors fail. Empty cache or context with no observed anchor
        // peers degrades to plain random selection.
        let mut shuffled: Vec<libp2p::PeerId> = peers
            .choose_multiple(&mut rand::thread_rng(), peers.len())
            .copied()
            .collect();
        let anchor_count = super::peers::partition_peers_anchor_first(
            &mut shuffled,
            &*self.state_access,
            &self.anchor_identities_for_context(&context_id),
        );
        if anchor_count > 0 {
            debug!(
                %context_id,
                anchor_peer_count = anchor_count,
                non_anchor_peer_count = shuffled.len() - anchor_count,
                "Preferring anchor peers for sync"
            );
        } else {
            debug!(
                %context_id,
                peer_count = shuffled.len(),
                "No anchor peers connected — falling back to random selection"
            );
        }
        for peer_id in &shuffled {
            if let Ok(result) = self.initiate_sync(context_id, *peer_id).await {
                return Ok(result);
            }
        }

        bail!("Failed to sync with any peer for context {}", context_id)
    }

    /// Returns the in-flight upgrade target application for `context_id`
    /// when an application upgrade/migration is pending on THIS node —
    /// i.e. the context's currently-bound application differs from its
    /// group's `target_application_id`. `None` when the context is
    /// already on its target (no pending upgrade) or any lookup fails.
    ///
    /// Used to gate context-STATE sync in both directions (outbound
    /// `initiate_sync_inner` and the inbound stream handler). While an
    /// upgrade is pending here, a peer that has ALREADY migrated must
    /// not reconcile its new-application-version state onto this node:
    /// HashComparison merges root entries by hash with no notion of
    /// application version, so it would overwrite the pre-upgrade state
    /// that this node's own (LazyOnAccess) migration must read as input
    /// — the migrate fn would then try to decode already-migrated bytes
    /// as the old shape and panic. This is the sync-side analogue of
    /// the write-gate.
    ///
    /// Only per-context state reconciliation is gated. Governance sync
    /// (the namespace DAG carrying the upgrade op itself) flows through
    /// a different path and is unaffected, so this node still learns
    /// about the upgrade and self-migrates on its next context access,
    /// after which this returns `None` and state sync resumes.
    ///
    /// Gate vs fence: this function provides COARSE, active-upgrade-window
    /// protection — it declines context-state sync while
    /// `current_app != target`. The sticky `cascade_hlc` recorded on the
    /// upgrade row (plus the post-`Completed` HLC fence in
    /// `calimero_context::hlc_fence`) provides FINER, long-tail protection
    /// that rejects late straggler / offline-writer state deltas even after
    /// the upgrade completes. The two mechanisms cover DISJOINT time windows
    /// (InProgress vs post-Completed), so there is no double-rejection. The
    /// fence's `None`-boundary bypass means a context that has not yet applied
    /// the cascade op is never fenced — this gate / lazy-upgrade handles it.
    fn pending_upgrade_target(
        &self,
        context_id: &ContextId,
    ) -> Option<calimero_primitives::application::ApplicationId> {
        let store = self.context_client.datastore_handle().into_inner();
        let ctx_meta = store
            .handle()
            .get(&calimero_store::key::ContextMeta::new(*context_id))
            .ok()
            .flatten()?;
        let current_app = ctx_meta.application.application_id();
        let group_id = calimero_context::group_store::get_group_for_context(&store, context_id)
            .ok()
            .flatten()?;
        let meta = MetaRepository::new(&store).load(&group_id).ok().flatten()?;
        let target = meta.target_application_id;
        // Only gate a context that is bound to a REAL application and differs
        // from the group target. A context with no app yet
        // (`current_app == ZERO`, e.g. a freshly-joined node still
        // bootstrapping its state) must be allowed to sync — gating it would
        // block the very state sync it needs to come up. Likewise `target ==
        // ZERO` means no upgrade is set.
        let zero = calimero_primitives::application::ZERO_APPLICATION_ID;
        (current_app != zero && target != zero && current_app != target).then_some(target)
    }

    /// Look up the trusted-anchor identity set for the group that owns
    /// `context_id` (Owner, Admins, ReadOnlyTee members). Returns an
    /// empty set on any failure — context not registered to a group,
    /// store read error, or no meta written yet. Callers fall back to
    /// plain random peer selection on an empty set.
    fn anchor_identities_for_context(
        &self,
        context_id: &ContextId,
    ) -> std::collections::BTreeSet<calimero_primitives::identity::PublicKey> {
        let store = self.context_client.datastore_handle().into_inner();
        let Ok(Some(group_id)) =
            calimero_context::group_store::get_group_for_context(&store, context_id)
        else {
            return std::collections::BTreeSet::new();
        };
        self.anchor_identities_for_group(&group_id)
    }

    /// Look up the trusted-anchor identity set for a group directly.
    /// Preferred over [`Self::anchor_identities_for_context`] when the
    /// caller already knows `group_id` — late-joiner nodes can have a
    /// missing context→group mapping, which makes the context-keyed
    /// lookup return an empty set even though the group's anchors are
    /// well-defined on the local node.
    fn anchor_identities_for_group(
        &self,
        group_id: &calimero_context_config::types::ContextGroupId,
    ) -> std::collections::BTreeSet<calimero_primitives::identity::PublicKey> {
        let store = self.context_client.datastore_handle().into_inner();
        MembershipRepository::new(&store)
            .trusted_anchors(group_id)
            .unwrap_or_default()
    }

    /// Find a peer that has state (non-zero root_hash and non-empty DAG heads)
    ///
    /// This is critical for bootstrapping newly joined nodes. Without this,
    /// uninitialized nodes may query other uninitialized nodes, resulting in
    /// all nodes remaining uninitialized.
    ///
    /// Peers are probed concurrently so a single slow/unreachable peer no
    /// longer stalls the entire discovery. The first peer to report state
    /// wins and remaining probes are cancelled when this function returns.
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

        let timeout_budget = self.sync_config.timeout / 6;
        let concurrency = self
            .sync_config
            .peer_state_probe_concurrency
            .min(peers.len())
            .max(1);

        debug!(
            %context_id,
            peer_count = peers.len(),
            concurrency,
            "Probing peers for state in parallel"
        );

        // Each probe opens a P2P stream, sends one `DagHeadsRequest`, and
        // reads the response. When we find a peer with state and return, the
        // remaining in-flight probes are dropped without sending a close
        // frame; libp2p's idle-timeout handles the cleanup, and the peer may
        // log a write-error if it was mid-response. This is an accepted
        // trade-off — the probe is read-only on the local node, so there is
        // no partial state to unwind, and adding an explicit graceful-close
        // path would require async work in `Drop`, which Rust does not
        // support cleanly.
        let mut probes = stream::iter(peers.iter().copied())
            .map(|peer_id| async move {
                let outcome = async {
                    let mut stream = self.sync_network.open_stream(peer_id).await?;

                    let request_msg = StreamMessage::Init {
                        context_id,
                        party_id: our_identity,
                        payload: InitPayload::DagHeadsRequest { context_id },
                        next_nonce: rand::thread_rng().gen(),
                    };

                    self.send(&mut stream, &request_msg, None).await?;

                    let Some(response) =
                        super::stream::recv(&mut stream, None, timeout_budget).await?
                    else {
                        return Ok::<_, eyre::Error>(None);
                    };

                    if let StreamMessage::Message {
                        payload:
                            MessagePayload::DagHeadsResponse {
                                dag_heads,
                                root_hash,
                            },
                        ..
                    } = response
                    {
                        // Peer has state if root_hash is not zeros (dag_heads may
                        // be empty for migrated/legacy contexts).
                        let has_state = *root_hash != [0; 32];
                        let heads_count = dag_heads.len();
                        debug!(
                            %context_id,
                            %peer_id,
                            heads_count,
                            %root_hash,
                            has_state,
                            "Received DAG heads from peer"
                        );
                        Ok(Some((has_state, heads_count, root_hash)))
                    } else {
                        Ok(None)
                    }
                }
                .await;

                (peer_id, outcome)
            })
            .buffer_unordered(concurrency);

        while let Some((peer_id, outcome)) = probes.next().await {
            match outcome {
                Ok(Some((true, heads_count, root_hash))) => {
                    info!(
                        %context_id,
                        %peer_id,
                        heads_count,
                        %root_hash,
                        "Found peer with state for bootstrapping"
                    );
                    return Ok(peer_id);
                }
                Ok(Some((false, _, _))) => {
                    debug!(%context_id, %peer_id, "peer reported no state");
                }
                Ok(None) => {
                    debug!(%context_id, %peer_id, "peer did not return DAG heads");
                }
                Err(e) => {
                    debug!(%context_id, %peer_id, error = %e, "peer probe failed");
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

        // Metrics: every sync attempt goes through this chokepoint, so
        // `sync_start / sync_complete / sync_failure` here covers every
        // protocol path. We don't yet know the protocol on entry — pass
        // "unknown"; the success arm overwrites with the protocol the
        // negotiated path actually chose.
        self.metrics()
            .record_sync_start(&context_id.to_string(), "unknown", "interval");

        let protocol = match self.initiate_sync_inner(context_id, peer_id).await {
            Ok(protocol) => protocol,
            Err(err) => {
                warn!(
                    %context_id,
                    %peer_id,
                    error = %err,
                    "Sync attempt failed for peer"
                );
                self.metrics().record_sync_failure(
                    &context_id.to_string(),
                    "unknown",
                    err.to_string().as_str(),
                );
                return Err(err);
            }
        };

        let took = start.elapsed();

        info!(%context_id, %peer_id, ?took, ?protocol, "Sync with peer completed successfully");

        // Use the variant-only `SyncProtocolKind` for the protocol label
        // so it matches the fixed `KNOWN_PROTOCOLS` set in
        // `PrometheusSyncMetrics::sanitize_protocol`. Formatting the
        // data-carrying `SyncProtocol` with `{:?}` would yield strings
        // like `HashComparison { root_hash: [...] }`
        // which never match the sanitiser and would label every sync
        // `protocol="unknown"`, breaking the per-protocol slicing on
        // `sync_successes_total` and `sync_duration_seconds`.
        //
        // `entities_transferred` is not threaded back to the sync manager
        // today; pass 0. The collector still records the duration histogram
        // and a sync_successes increment, which are the two most useful
        // signals on a dashboard.
        self.metrics().record_sync_complete(
            &context_id.to_string(),
            &format!("{:?}", protocol.kind()),
            took,
            0,
        );

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
    ///
    /// #2422 Option 4: when the responder replies with
    /// [`StreamMessage::NotMaterialized`], convert it to a
    /// [`PeerNotMaterialized`] error so the apply-session-result
    /// classifier can treat it as benign (no `on_failure`, no
    /// exponential backoff). The conversion happens here — the
    /// single common recv path — so individual protocol decoders
    /// (HashComparison, LevelWise, etc.) don't each have to grow a
    /// NotMaterialized arm.
    pub(super) async fn recv(
        &self,
        stream: &mut Stream,
        shared_key: Option<(SharedKey, Nonce)>,
    ) -> eyre::Result<Option<StreamMessage<'static>>> {
        let budget = self.sync_config.timeout / 3;
        let msg = super::stream::recv(stream, shared_key, budget).await?;
        if matches!(msg, Some(StreamMessage::NotMaterialized)) {
            return Err(eyre::Error::new(PeerNotMaterialized));
        }
        Ok(msg)
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
            let app_config = self
                .context_client
                .get_context_application(context_id)
                .await?;
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
            let app_config = self
                .context_client
                .get_context_application(context_id)
                .await?;
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
            let app_config = self
                .context_client
                .get_context_application(context_id)
                .await?;
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

        // Get source from context config (use cached if available, otherwise fetch)
        let source = self
            .get_application_source(context_id, app_config_opt)
            .await?;

        let installed_app_id = if is_bundle {
            self.node_client
                .install_application_from_bundle_blob(blob_id, &source)
                .await
                .map_err(|e| {
                    eyre::eyre!(
                        "Failed to install bundle application from blob {}: {}",
                        blob_id,
                        e
                    )
                })?
        } else {
            // For non-bundle apps, write ApplicationMeta directly under the
            // known application_id rather than re-deriving it via
            // install_application (which hashes source+metadata and would
            // produce a different ID than the original installer used).
            let size = blob_bytes.len() as u64;
            let mut handle = self.context_client.datastore_handle();
            handle.put(
                &calimero_store::key::ApplicationMeta::new(context.application_id),
                &calimero_store::types::ApplicationMeta::new(
                    calimero_store::key::BlobMeta::new(*blob_id),
                    size,
                    source.to_string().into_boxed_str(),
                    Box::default(),
                    calimero_store::key::BlobMeta::new(calimero_primitives::blobs::BlobId::from(
                        [0u8; 32],
                    )),
                    "unknown".to_owned().into_boxed_str(),
                    "0.0.0".to_owned().into_boxed_str(),
                    String::new().into_boxed_str(),
                ),
            )?;
            context.application_id
        };

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
            info!(
                %context_id,
                %chosen_peer,
                is_uninitialized,
                has_incomplete_sync,
                "Node needs snapshot sync, checking if peer has state"
            );

            // Query peer's state to decide sync strategy
            let peer_state = self
                .query_peer_dag_state(context_id, chosen_peer, our_identity, stream)
                .await?;

            match peer_state {
                Some((peer_root_hash, _peer_dag_heads)) if *peer_root_hash != [0; 32] => {
                    // Peer has state - use snapshot sync for efficient bootstrap
                    info!(
                        %context_id,
                        %chosen_peer,
                        peer_root_hash = %peer_root_hash,
                        "Peer has state, using snapshot sync for bootstrap"
                    );

                    // Note: request_snapshot_sync opens its own stream, existing stream
                    // will be closed when this function returns
                    // force=false: This is bootstrap for uninitialized nodes
                    match self
                        .request_snapshot_sync(context_id, chosen_peer, false)
                        .await
                        .wrap_err("snapshot sync")
                    {
                        Ok(result) => {
                            info!(
                                %context_id,
                                %chosen_peer,
                                applied_records = result.applied_records,
                                boundary_root_hash = %result.boundary_root_hash,
                                dag_heads_count = result.dag_heads.len(),
                                "Snapshot sync completed successfully"
                            );

                            // CRITICAL: Add snapshot boundary checkpoints to DAG
                            // This ensures that when new deltas arrive referencing the
                            // snapshot boundary heads as parents, the DAG accepts them.
                            if !result.dag_heads.is_empty() {
                                let context_client = self.context_client.clone();
                                let (delta_store, _was_newly_created) =
                                    self.state_access.get_or_register_delta_store(
                                        context_id,
                                        Box::new(move || {
                                            crate::delta_store::DeltaStore::new(
                                                [0u8; 32],
                                                context_client,
                                                context_id,
                                                our_identity,
                                            )
                                        }),
                                    );

                                let checkpoints_added = delta_store
                                    .add_snapshot_checkpoints(
                                        result.dag_heads.clone(),
                                        *result.boundary_root_hash,
                                    )
                                    .await;

                                info!(
                                    %context_id,
                                    checkpoints_added,
                                    "Added snapshot boundary checkpoints to DAG"
                                );

                                match self.sync_network.open_stream(chosen_peer).await {
                                    Ok(mut fine_stream) => {
                                        if let Err(e) = self
                                            .fine_sync_from_boundary(
                                                context_id,
                                                chosen_peer,
                                                our_identity,
                                                &mut fine_stream,
                                            )
                                            .await
                                        {
                                            warn!(
                                                %context_id,
                                                %chosen_peer,
                                                error = %e,
                                                "Fine-sync after snapshot failed, state may be slightly behind"
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        warn!(
                                            %context_id,
                                            %chosen_peer,
                                            error = %e,
                                            "Fine-sync stream open failed, state may be slightly behind"
                                        );
                                    }
                                }
                            }

                            // Replay any buffered deltas (from uninitialized context period)
                            // This ensures handlers execute for deltas that arrived before sync completed
                            if let Some(buffered_deltas) =
                                self.state_access.end_sync_session(&context_id)
                            {
                                let buffered_count = buffered_deltas.len();
                                if buffered_count > 0 {
                                    info!(
                                        %context_id,
                                        buffered_count,
                                        "Replaying buffered deltas after snapshot sync (bootstrap path)"
                                    );
                                    self.replay_buffered_deltas(
                                        context_id,
                                        our_identity,
                                        buffered_deltas,
                                        chosen_peer,
                                    )
                                    .await;
                                }
                            }

                            return Ok(Some(SyncProtocol::Snapshot {
                                compressed: false,
                                verified: true,
                            }));
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
        if let Some(delta_store) = self.state_access.delta_store(&context_id) {
            // NOTE: previously called `load_persisted_deltas()` here to
            // catch locally-created deltas from execute.rs that are in
            // the DB but not in the in-memory DAG. That rescan was
            // ~21% of CPU (pre #2244) and ~6% after. execute.rs and
            // create_context.rs now notify the node-side drainer via
            // `NodeClient::notify_local_applied_delta`, keeping the
            // DAG current without the per-sync full-column scan.
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
                    .await
                    .wrap_err("request DAG heads and sync")?;

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
            // Build handshakes for protocol selection (CIP §2.3)
            // Uses shared functions from calimero_node_primitives::sync::state_machine
            let local_hs = self.build_local_handshake(context);
            let remote_hs = Self::build_remote_handshake(peer_root_hash, &peer_dag_heads);

            // Select optimal sync protocol based on state comparison
            let selection = select_protocol(&local_hs, &remote_hs);

            info!(
                %context_id,
                %chosen_peer,
                protocol = ?selection.protocol,
                reason = %selection.reason,
                local_root = %context.root_hash,
                remote_root = %peer_root_hash,
                local_entities = local_hs.entity_count,
                remote_entities = remote_hs.entity_count,
                "Protocol selected"
            );

            return self
                .protocol_selector
                .execute(
                    self,
                    selection,
                    context_id,
                    chosen_peer,
                    our_identity,
                    &context.root_hash,
                    &peer_root_hash,
                    stream,
                )
                .await;
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
        let sync_start = Instant::now();

        let mut context = self
            .context_client
            .sync_context_config(context_id, None)
            .await?;

        let is_uninitialized = *context.root_hash == [0; 32];
        info!(
            %context_id,
            %chosen_peer,
            is_uninitialized,
            root_hash = %context.root_hash,
            dag_heads_count = context.dag_heads.len(),
            application_id = %context.application_id,
            "Starting sync session"
        );

        // Sync-gate: if an application upgrade is pending on this context
        // (our bound app != the group's target app), do NOT reconcile
        // state with a peer — it may have already migrated, and merging
        // its new-version state here would overwrite the pre-upgrade
        // state our own LazyOnAccess migration must read as input. Skip
        // as a clean no-op; we self-migrate on next access, after which
        // the gate lifts. See `pending_upgrade_target`.
        if let Some(target) = self.pending_upgrade_target(&context_id) {
            info!(
                %context_id,
                %chosen_peer,
                current_app = %context.application_id,
                target_app = %target,
                "Skipping context-state sync: application upgrade pending (gate); \
                 node self-migrates on next access before reconciling"
            );
            return Ok(SyncProtocol::None);
        }

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

        let mut stream = self
            .sync_network
            .open_stream(chosen_peer)
            .await
            .wrap_err("open stream for sync")?;

        // Key share phase removed — group key envelopes handle key distribution.
        let key_share_elapsed = std::time::Duration::ZERO;
        debug!(
            %context_id,
            %chosen_peer,
            ?key_share_elapsed,
            "Phase 1/3 complete: key share"
        );

        // Phase 2: Blob share (if needed)
        if !self.node_client.has_blob(&blob_id)? {
            let phase_start = Instant::now();
            // Get size from application config if we don't have application yet
            let size = self
                .get_application_size(&context_id, &application, &app_config_opt)
                .await?;

            self.initiate_blob_share_process(&context, our_identity, blob_id, size, &mut stream)
                .await
                .wrap_err("blob share")?;

            let blob_share_elapsed = phase_start.elapsed();
            debug!(
                %context_id,
                %chosen_peer,
                ?blob_share_elapsed,
                "Phase 2/3 complete: blob share"
            );

            // After blob sharing, try to install application if it doesn't exist
            // or if we only have a stub (size==0 from join_context bootstrap)
            let needs_install =
                application.is_none() || application.as_ref().is_some_and(|app| app.size == 0);
            if needs_install {
                self.install_bundle_after_blob_sharing(
                    &context_id,
                    &blob_id,
                    &app_config_opt,
                    &mut context,
                    &mut application,
                )
                .await
                .wrap_err("install bundle after blob share")?;
            }
        }

        let Some(_application) = application else {
            if context.application_id
                == calimero_primitives::application::ApplicationId::from([0u8; 32])
            {
                bail!("context has placeholder application ID — waiting for governance op to resolve it");
            }
            bail!("application not found: {}", context.application_id);
        };

        // Phase 3: DAG synchronization (if needed — uninitialized or incomplete DAG)
        let phase_start = Instant::now();
        if let Some(result) = self
            .handle_dag_sync(context_id, &context, chosen_peer, our_identity, &mut stream)
            .await
            .wrap_err("DAG sync")?
        {
            let dag_sync_elapsed = phase_start.elapsed();
            let total_elapsed = sync_start.elapsed();
            info!(
                %context_id,
                %chosen_peer,
                ?key_share_elapsed,
                ?dag_sync_elapsed,
                ?total_elapsed,
                protocol = ?result,
                "Sync session complete (DAG sync performed)"
            );
            return Ok(result);
        }

        let total_elapsed = sync_start.elapsed();
        // Otherwise, DAG-based sync happens automatically via BroadcastMessage::StateDelta
        debug!(
            %context_id,
            %chosen_peer,
            ?key_share_elapsed,
            ?total_elapsed,
            "Sync session complete: node is in sync, no active protocol needed"
        );
        Ok(SyncProtocol::None)
    }

    /// Request peer's DAG heads and sync all missing deltas
    async fn request_dag_heads_and_sync(
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
                let (delta_store_ref, is_new) = {
                    let context_client = self.context_client.clone();
                    self.state_access.get_or_register_delta_store(
                        context_id,
                        Box::new(move || {
                            crate::delta_store::DeltaStore::new(
                                [0u8; 32],
                                context_client,
                                context_id,
                                our_identity,
                            )
                        }),
                    )
                };

                // The previous revision ran `load_persisted_deltas`
                // unconditionally here on every sync — the rescan
                // dominated the hot path. execute.rs now notifies the
                // node-side drainer directly, so warm stores don't
                // need rehydration. But when *this* path is the first
                // to create the DeltaStore for a context (fresh boot,
                // sync arrives before the first local execute), the
                // in-memory DAG is empty and we still need a one-time
                // load so `get_delta` can serve peers and missing-
                // parent queries have the right picture.
                if is_new {
                    if let Err(e) = delta_store_ref.load_persisted_deltas().await {
                        warn!(
                            ?e,
                            %context_id,
                            "Failed to hydrate freshly-created DeltaStore from DB"
                        );
                    }
                }

                // Phase 1: Request and add ALL DAG heads
                //
                // Count outcomes so we can detect the silent-no-op case:
                // a peer advertised N heads, every one was rejected by
                // the signature/membership/group-id checks, and we
                // therefore added zero deltas. Without the
                // counters below, `missing_ids` would be empty after
                // the loop and the fast-return at 1979 would claim
                // success despite no progress — the divergence would
                // persist and the caller would back off as if it had
                // already converged. `heads_attempted` excludes the
                // DeltaNotFound case (peer doesn't have it, not a
                // rejection); `heads_admitted` includes the
                // successful `add_delta` path only.
                let mut heads_attempted: u32 = 0;
                let mut heads_admitted: u32 = 0;
                // Hoist the datastore handle outside the loop —
                // `datastore_handle().into_inner()` clones an `Arc`
                // and can take a brief lock; per-iteration creation
                // showed up in reviewer profiling as redundant since
                // every head reuses the same handle. The handle is
                // borrowed read-only by the membership check and the
                // group-id parity check; both can share.
                let datastore_for_heads = self.context_client.datastore_handle().into_inner();
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
                            payload:
                                MessagePayload::DeltaResponse {
                                    delta,
                                    author_id: response_author,
                                    governance_position_blob,
                                    // Peer claimed to have the delta; count the
                                    // attempt regardless of whether the verify
                                    // chain ultimately accepts it. The
                                    // DeltaNotFound arm below is *not* an
                                    // attempt — the peer simply doesn't have
                                    // it, so it can't be a "rejection".
                                    delta_signature: response_delta_signature,
                                },
                            ..
                        }) => {
                            heads_attempted = heads_attempted.saturating_add(1);
                            // Deserialize and add to DAG
                            let storage_delta: calimero_storage::delta::CausalDelta =
                                borsh::from_slice(&delta)?;

                            // Sanity check: peer returned the head we
                            // requested. A buggy or malicious peer
                            // could substitute a different authorized
                            // delta in response. The envelope signature
                            // binds `storage_delta.id`, not `head_id`,
                            // so without this guard a peer could swap
                            // a valid delta for another and slip it
                            // into our DAG under the wrong slot —
                            // parity with the parent-fetch path's
                            // sanity check, same security rationale.
                            if storage_delta.id != *head_id {
                                warn!(
                                    %context_id,
                                    requested = ?head_id,
                                    received = ?storage_delta.id,
                                    "DAG head pull: peer returned a different delta id than requested, dropping"
                                );
                                continue;
                            }

                            // Apply-time cross-DAG membership check —
                            // parity with the gossip-path check in
                            // `handle_state_delta`. `response_author` is
                            // required on the wire (the responder filters
                            // out rows without an author claim, returning
                            // `DeltaNotFound` so the initiator can fall
                            // back to a verifiable path). No legacy-accept
                            // escape hatch here.
                            //
                            // `governance_position` is `Option` because
                            // non-group contexts legitimately have no
                            // cut to cite. In that case the membership
                            // check is skipped (nothing to verify against
                            // — context isn't governed by a group
                            // membership DAG), and the per-action
                            // signatures inside `apply_action` remain
                            // the auth primitive.
                            let author = response_author;

                            // Genesis carve-out: the responder serves
                            // the genesis delta with the all-zeros
                            // sentinel `author_id` because the wire
                            // requires an author but genesis predates
                            // any governance op. Skip every
                            // author-keyed check — none of them apply
                            // to genesis. Persist directly via the
                            // same add_delta path; gossip never sees
                            // genesis (it's installed at context
                            // creation), so the only way late joiners
                            // backfill it is via this catchup path.
                            if crate::sync::delta_request::is_genesis_author_sentinel(&author) {
                                debug!(
                                    %context_id,
                                    head_id = ?head_id,
                                    "DAG head pull: accepting genesis delta via author sentinel"
                                );
                                let dag_delta = calimero_dag::CausalDelta {
                                    id: storage_delta.id,
                                    parents: storage_delta.parents.clone(),
                                    payload: storage_delta.actions,
                                    hlc: storage_delta.hlc,
                                    expected_root_hash: storage_delta.expected_root_hash,
                                    kind: calimero_dag::DeltaKind::Regular,
                                };
                                if let Err(e) =
                                    delta_store_ref.add_delta(dag_delta, None, None, None).await
                                {
                                    warn!(
                                        ?e,
                                        %context_id,
                                        head_id = ?head_id,
                                        "Failed to add genesis DAG head delta"
                                    );
                                } else {
                                    heads_admitted = heads_admitted.saturating_add(1);
                                    info!(
                                        %context_id,
                                        head_id = ?head_id,
                                        "Successfully added genesis DAG head delta"
                                    );
                                }
                                continue;
                            }

                            let pos = match governance_position_blob
                                .as_deref()
                                .map(
                                    borsh::from_slice::<
                                        calimero_context_config::types::GovernancePosition,
                                    >,
                                )
                                .transpose()
                            {
                                Ok(p) => p,
                                Err(e) => {
                                    // Malformed governance_position
                                    // blob on a single delta shouldn't
                                    // poison the whole DAG-catchup
                                    // batch — skip this delta and
                                    // continue. Other deltas may still
                                    // converge; this one will retry on
                                    // the next sync tick.
                                    warn!(
                                        %context_id,
                                        %author,
                                        head_id = ?head_id,
                                        %e,
                                        "DAG-catchup: failed to decode governance_position \
                                         from peer; skipping this delta and continuing"
                                    );
                                    continue;
                                }
                            };
                            // Per-delta envelope signature verification —
                            // parity with `apply_authorized_state_delta`'s
                            // gossip-path check. Runs BEFORE the cross-DAG
                            // membership check because that check keys off
                            // `author`; we have to establish the authorship
                            // claim is genuine before asking whether the
                            // claimed author is authorized. `None` is
                            // tolerated only for legacy rows authored
                            // before envelope signing landed and for
                            // snapshot checkpoints / genesis rows that
                            // have no author signature to record; every
                            // freshly-authored delta (every output of
                            // `internal_execute`) carries `Some(_)` and
                            // MUST verify.
                            if let Some(ref sig) = response_delta_signature {
                                if let Err(err) = calimero_node_primitives::sync::delta_auth::verify_delta_signature(
                                    context_id,
                                    storage_delta.id,
                                    author,
                                    pos.as_ref(),
                                    sig,
                                ) {
                                    warn!(
                                        %context_id,
                                        %author,
                                        head_id = ?head_id,
                                        %err,
                                        "DAG-catchup: rejecting delta — envelope signature \
                                         verification failed"
                                    );
                                    continue;
                                }
                            }

                            // Anti-bypass parity with the gossip path: before
                            // running `membership_status_at`, confirm the
                            // claimed governance position's `group_id`
                            // actually matches the context's owning group
                            // (or, for non-group contexts, that no position
                            // is claimed). Without this:
                            //   * `GroupContextNoPosition` — a group context
                            //     would accept a delta with no position at
                            //     all, silently skipping the membership check
                            //     entirely (the `if let Some(pos)` branch
                            //     below would just fall through).
                            //   * `Mismatch` — an attacker could sign a
                            //     position for a group they're a member of
                            //     and attach it to a delta targeted at a
                            //     different context owned by a different
                            //     group, and `membership_status_at` would
                            //     still pass against the spoofed group.
                            //   * `NonGroupContextWithPosition` — symmetric.
                            // The gossip path catches all three via
                            // `verify_position_group_id_matches_context`; we
                            // share the same helper so the match table
                            // stays in lockstep across the two code paths.
                            {
                                use crate::handlers::state_delta::{
                                    verify_position_group_id_matches_context, GroupIdCheck,
                                };
                                match verify_position_group_id_matches_context(
                                    &datastore_for_heads,
                                    &context_id,
                                    pos.as_ref().map(|p| p.group_id),
                                ) {
                                    GroupIdCheck::Match | GroupIdCheck::NonGroupOk => {}
                                    GroupIdCheck::GroupContextNoPosition { owning } => {
                                        warn!(
                                            %context_id,
                                            %author,
                                            head_id = ?head_id,
                                            owning_group = ?owning,
                                            "DAG-catchup: rejecting delta — context is owned \
                                             by a group but delta carries no \
                                             governance_position (parity gap with gossip path)"
                                        );
                                        continue;
                                    }
                                    GroupIdCheck::NonGroupContextWithPosition { claimed } => {
                                        warn!(
                                            %context_id,
                                            %author,
                                            head_id = ?head_id,
                                            claimed_group = ?claimed,
                                            "DAG-catchup: rejecting delta — delta claims a \
                                             governance position but context is not in any \
                                             group"
                                        );
                                        continue;
                                    }
                                    GroupIdCheck::Mismatch { owning, claimed } => {
                                        warn!(
                                            %context_id,
                                            %author,
                                            head_id = ?head_id,
                                            owning_group = ?owning,
                                            claimed_group = ?claimed,
                                            "DAG-catchup: rejecting delta — governance_position \
                                             references a different group than the context's \
                                             owning group"
                                        );
                                        continue;
                                    }
                                    GroupIdCheck::LookupError(err) => {
                                        warn!(
                                            %context_id,
                                            %author,
                                            head_id = ?head_id,
                                            %err,
                                            "DAG-catchup: skipping delta — group lookup failed \
                                             during anti-bypass check"
                                        );
                                        continue;
                                    }
                                }
                            }

                            // ReadOnly check — parity with the gossip
                            // apply path. `membership_status_at` treats
                            // ReadOnly as `Member(ReadOnly)`, so a
                            // ReadOnly identity's delta would slip past
                            // the cross-DAG check on the catchup path
                            // even though gossip rejects it. Mirror the
                            // gate `apply_authorized_state_delta` uses.
                            if NamespaceRepository::new(&datastore_for_heads)
                                .is_read_only_for_context(&context_id, &author)
                                .unwrap_or(false)
                            {
                                warn!(
                                    %context_id,
                                    %author,
                                    head_id = ?head_id,
                                    "DAG-catchup: rejecting delta from ReadOnly member"
                                );
                                continue;
                            }

                            if let Some(ref pos) = pos {
                                use calimero_context::group_store::{
                                    membership_status_at, MembershipStatus,
                                };
                                match membership_status_at(&datastore_for_heads, &author, pos) {
                                    Ok(MembershipStatus::Member(_)) => {
                                        // Authorized at the cited cut — proceed.
                                    }
                                    Ok(MembershipStatus::Removed { last_role }) => {
                                        warn!(
                                            %context_id,
                                            %author,
                                            head_id = ?head_id,
                                            last_role = ?last_role,
                                            "DAG-catchup: rejecting delta — author was \
                                             removed at the cited governance cut"
                                        );
                                        continue;
                                    }
                                    Ok(MembershipStatus::NeverMember) => {
                                        warn!(
                                            %context_id,
                                            %author,
                                            head_id = ?head_id,
                                            "DAG-catchup: rejecting delta — author was \
                                             never a member at the cited governance cut"
                                        );
                                        continue;
                                    }
                                    Ok(MembershipStatus::Unknown { needed }) => {
                                        // Buffering this delta the way the gossip path
                                        // does would close the loop, but the DAG-catchup
                                        // dispatch flow doesn't have the buffer plumbing
                                        // wired yet. Skipping for now means the next sync
                                        // tick will re-attempt once governance state has
                                        // caught up via gossip; safer than admitting an
                                        // unverified delta on the catch-up path.
                                        warn!(
                                            %context_id,
                                            %author,
                                            head_id = ?head_id,
                                            needed = ?needed,
                                            "DAG-catchup: skipping delta — governance \
                                             cut not locally known; will re-attempt on \
                                             next sync tick"
                                        );
                                        continue;
                                    }
                                    Err(e) => {
                                        warn!(
                                            %context_id,
                                            %author,
                                            head_id = ?head_id,
                                            error = %e,
                                            "DAG-catchup: skipping delta — \
                                             membership_status_at failed"
                                        );
                                        continue;
                                    }
                                }
                            }

                            let dag_delta = calimero_dag::CausalDelta {
                                id: storage_delta.id,
                                parents: storage_delta.parents,
                                payload: storage_delta.actions,
                                hlc: storage_delta.hlc,
                                expected_root_hash: storage_delta.expected_root_hash,
                                kind: calimero_dag::DeltaKind::Regular,
                            };

                            // Persist with the wire-received author +
                            // governance position so this node can in
                            // turn serve verifiable DAG-catchup responses
                            // to other peers that ask for the same delta.
                            let persisted_gov_blob =
                                governance_position_blob.as_ref().map(|c| c.to_vec());
                            if let Err(e) = delta_store_ref
                                .add_delta(
                                    dag_delta,
                                    Some(author),
                                    persisted_gov_blob,
                                    response_delta_signature,
                                )
                                .await
                            {
                                warn!(
                                    ?e,
                                    %context_id,
                                    head_id = ?head_id,
                                    "Failed to add DAG head delta"
                                );
                            } else {
                                heads_admitted = heads_admitted.saturating_add(1);
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
                                .fallback_to_snapshot_sync(context_id, our_identity, peer_id)
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

                // Detect "all-rejected" silent no-op: the peer advertised
                // heads, every single one was rejected by the verify chain
                // (signature / group-id / membership), and so we admitted
                // zero deltas. If we let the code fall through to the
                // `missing_ids.is_empty()` fast-return below, catchup would
                // claim `Ok(DeltaSync { missing_delta_ids: vec![] })` —
                // i.e. "success" — even though the divergence remains.
                // Bail loudly here so the caller knows to back off and
                // either try another peer or escalate to snapshot sync,
                // matching how the rest of `handle_dag_sync`'s error paths
                // propagate.
                if heads_attempted > 0 && heads_admitted == 0 {
                    bail!(
                        "DAG-catchup made no progress against peer {peer_id}: \
                         all {heads_attempted} advertised head deltas were rejected \
                         by the apply-time verify chain (signature / group-id / \
                         membership). Reporting as failure rather than claiming \
                         convergence — caller should back off and retry against \
                         another peer or fall back to snapshot sync."
                    );
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

                // Steady-state: the initial DAG-heads response matched local
                // state, so there are no missing parents to chase. Skip the
                // entire retry-and-final-check machinery on the common path.
                if missing_result.missing_ids.is_empty() {
                    return Ok(SyncProtocol::DeltaSync {
                        missing_delta_ids: vec![],
                    });
                }

                info!(
                    %context_id,
                    missing_count = missing_result.missing_ids.len(),
                    "DAG heads have missing parents, requesting them recursively"
                );

                // First attempt: the peer that served DAG heads.
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
                        "Failed to request missing parent deltas from initial peer"
                    );
                }

                // Cross-peer fallback for cold-start race (#2198): if the
                // initial peer did not resolve every missing parent, iterate
                // other mesh peers for this context until the DAG is whole
                // or the retry budget is exhausted.
                let topic = TopicHash::from_raw(context_id);
                let mut budget = super::parent_pull::ParentPullBudget::new(
                    peer_id,
                    self.sync_config.parent_pull_additional_peers,
                    self.sync_config.parent_pull_budget,
                );
                let mut mesh_peers = self.sync_network.subscribed_peers(topic.clone()).await;

                loop {
                    let after = delta_store_ref.get_missing_parents().await;
                    if after.missing_ids.is_empty() {
                        break; // fully resolved
                    }

                    let next_peer = match budget.next(&mesh_peers) {
                        super::parent_pull::NextPeer::Peer(p) => p,
                        super::parent_pull::NextPeer::RefetchMesh => {
                            mesh_peers = self.sync_network.subscribed_peers(topic.clone()).await;
                            budget.record_refetch();
                            match budget.next(&mesh_peers) {
                                super::parent_pull::NextPeer::Peer(p) => p,
                                other => {
                                    debug!(
                                        %context_id,
                                        ?other,
                                        "no additional mesh peers available for parent pull"
                                    );
                                    break;
                                }
                            }
                        }
                        super::parent_pull::NextPeer::BudgetExhausted => {
                            warn!(
                                %context_id,
                                "parent-pull budget exhausted"
                            );
                            break;
                        }
                        super::parent_pull::NextPeer::MaxPeersReached
                        | super::parent_pull::NextPeer::NoMorePeers => break,
                    };

                    budget.record_attempt(next_peer);

                    info!(
                        %context_id,
                        ?next_peer,
                        attempt = budget.attempts(),
                        still_missing = after.missing_ids.len(),
                        "retrying missing-parent fetch against additional mesh peer"
                    );

                    if let Err(e) = self
                        .request_missing_deltas(
                            context_id,
                            after.missing_ids,
                            next_peer,
                            delta_store_ref.clone(),
                            our_identity,
                        )
                        .await
                    {
                        warn!(
                            ?e,
                            %context_id,
                            ?next_peer,
                            "cross-peer parent-pull attempt failed"
                        );
                    }
                }

                // Final check: if pending parents still remain, the sync did
                // NOT fully restore the DAG. Return an error so the caller
                // (e.g. join_context) surfaces a real failure instead of
                // silent success on a partially-applied DAG.
                let final_missing = delta_store_ref.get_missing_parents().await;
                if !final_missing.missing_ids.is_empty() {
                    warn!(
                        %context_id,
                        remaining = final_missing.missing_ids.len(),
                        peer_attempts = budget.total_attempts(),
                        "DAG sync ended with unresolved missing parents"
                    );
                    bail!(
                        "pending parents unresolved for context {}: {} remaining after {} peer attempt(s)",
                        context_id,
                        final_missing.missing_ids.len(),
                        budget.total_attempts(),
                    );
                }

                // Success: DAG is fully resolved.
                Ok(SyncProtocol::DeltaSync {
                    missing_delta_ids: vec![],
                })
            }
            _ => {
                warn!(%context_id, "Unexpected response to DAG heads request, trying next peer");
                Ok(SyncProtocol::None)
            }
        }
    }

    /// Fall back to full snapshot sync when delta sync is not possible.
    ///
    /// Implements Invariant I6: Deltas received during sync are buffered and
    /// replayed after sync completes. On error, buffered deltas are discarded
    /// via `cancel_sync_session()`.
    async fn fallback_to_snapshot_sync(
        &self,
        context_id: ContextId,
        our_identity: PublicKey,
        peer_id: PeerId,
    ) -> eyre::Result<SyncProtocol> {
        info!(%context_id, %peer_id, "Initiating snapshot sync");

        // Start buffering deltas that arrive during snapshot sync (Invariant I6)
        // Use current time as sync start HLC
        let sync_start_hlc = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        self.node_state
            .start_sync_session(context_id, sync_start_hlc);

        // force=false: Enforce Invariant I5 - only allow snapshot on fresh nodes.
        // If the node has state, this will fail, which is correct - divergence
        // or pruned history on initialized nodes cannot be safely resolved via
        // snapshot overwrite. CRDT merge must be used instead.
        let result = match self.request_snapshot_sync(context_id, peer_id, false).await {
            Ok(r) => r,
            Err(e) => {
                // Cancel sync session on failure - discard buffered deltas
                // since the context state is inconsistent
                self.state_access.cancel_sync_session(&context_id);
                return Err(e);
            }
        };
        info!(%context_id, records = result.applied_records, "Snapshot sync completed");

        // End buffering and get any deltas that arrived during sync
        let buffered_deltas = self.state_access.end_sync_session(&context_id);
        let buffered_count = buffered_deltas.as_ref().map_or(0, Vec::len);

        if buffered_count > 0 {
            info!(
                %context_id,
                buffered_count,
                "Replaying buffered deltas after snapshot sync"
            );

            // Replay buffered deltas - now that context is initialized, we can process them
            if let Some(deltas) = buffered_deltas {
                self.replay_buffered_deltas(context_id, our_identity, deltas, peer_id)
                    .await;
            }
        }

        // Fine-sync to catch any deltas since the snapshot boundary
        if !result.dag_heads.is_empty() {
            let mut stream = self.sync_network.open_stream(peer_id).await?;
            if let Err(e) = self
                .fine_sync_from_boundary(context_id, peer_id, our_identity, &mut stream)
                .await
            {
                warn!(?e, %context_id, "Fine-sync failed, state may be slightly behind");
            }
        }

        Ok(SyncProtocol::Snapshot {
            compressed: false,
            verified: true,
        })
    }

    /// Replay buffered deltas after snapshot sync completes.
    ///
    /// This ensures that:
    /// 1. Deltas arriving during sync aren't lost
    /// 2. Event handlers execute for buffered deltas
    /// 3. Ancestor deltas (whose state is covered by checkpoint) get handlers executed
    async fn replay_buffered_deltas(
        &self,
        context_id: ContextId,
        our_identity: PublicKey,
        mut deltas: Vec<calimero_node_primitives::delta_buffer::BufferedDelta>,
        _fallback_peer: PeerId,
    ) {
        use crate::handlers::state_delta::{replay_buffered_delta, ReplayBufferedDeltaInput};
        use std::collections::{HashMap, HashSet};

        // #2319 determinism: deltas land in the buffer in gossipsub
        // arrival order, which differs node-to-node — replaying them in
        // that order makes two nodes apply *concurrent* deltas to storage
        // in different sequences, which (for any merge that isn't
        // perfectly order-independent) yields a different Merkle root for
        // the same delta set. Replay in a canonical, causally-consistent
        // order — HLC, then delta id as a tiebreaker — so every node
        // applies the same sequence. (The DAG cascade still re-orders for
        // genuine causal dependencies; this only pins the order of
        // concurrent ones.)
        deltas.sort_by(|a, b| a.hlc.cmp(&b.hlc).then_with(|| a.id.cmp(&b.id)));

        // Build a set of IDs that are "covered" by the snapshot
        // This includes:
        // 1. Deltas that match checkpoints directly
        // 2. Deltas that are ancestors of checkpoints (their state is included in snapshot)
        let mut covered_delta_ids: HashSet<[u8; 32]> = HashSet::new();

        // Get the delta store to check for existing checkpoints.
        // If this path is the first to create the DeltaStore, hydrate
        // from DB once — incremental updates via execute.rs handle the
        // warm-store case, but a fresh store here would otherwise miss
        // everything on disk and we'd later fail to match checkpoints.
        let (delta_store, is_new) = {
            let context_client = self.context_client.clone();
            self.state_access.get_or_register_delta_store(
                context_id,
                Box::new(move || {
                    crate::delta_store::DeltaStore::new(
                        [0u8; 32],
                        context_client,
                        context_id,
                        our_identity,
                    )
                }),
            )
        };
        if is_new {
            if let Err(e) = delta_store.load_persisted_deltas().await {
                warn!(
                    ?e,
                    %context_id,
                    "Failed to hydrate freshly-created DeltaStore from DB"
                );
            }
        }

        // Build parent -> children map from buffered deltas
        let mut parent_to_children: HashMap<[u8; 32], Vec<[u8; 32]>> = HashMap::new();
        for buffered in &deltas {
            for parent in &buffered.parents {
                parent_to_children
                    .entry(*parent)
                    .or_default()
                    .push(buffered.id);
            }
        }

        // Identify which buffered deltas match existing checkpoints
        let mut checkpoint_matches: Vec<[u8; 32]> = Vec::new();
        for buffered in &deltas {
            if delta_store.dag_has_delta_applied(&buffered.id).await {
                checkpoint_matches.push(buffered.id);
                covered_delta_ids.insert(buffered.id);
            }
        }

        // Propagate "covered" status backwards through the parent chain
        // If delta D has a child C that is covered, then D is also covered
        // (D's state is included in C's checkpoint)
        let delta_ids: HashSet<[u8; 32]> = deltas.iter().map(|d| d.id).collect();
        let delta_parents: HashMap<[u8; 32], Vec<[u8; 32]>> =
            deltas.iter().map(|d| (d.id, d.parents.clone())).collect();

        // BFS backwards from checkpoint matches
        let mut queue: std::collections::VecDeque<[u8; 32]> =
            checkpoint_matches.iter().copied().collect();
        while let Some(child_id) = queue.pop_front() {
            // Get parents of this delta (if it's one of our buffered deltas)
            if let Some(parents) = delta_parents.get(&child_id) {
                for parent_id in parents {
                    // If parent is also a buffered delta and not yet covered
                    if delta_ids.contains(parent_id) && !covered_delta_ids.contains(parent_id) {
                        covered_delta_ids.insert(*parent_id);
                        queue.push_back(*parent_id);
                    }
                }
            }
        }

        if !covered_delta_ids.is_empty() {
            info!(
                %context_id,
                covered_count = covered_delta_ids.len(),
                checkpoint_matches = checkpoint_matches.len(),
                total_buffered = deltas.len(),
                "Identified buffered deltas covered by snapshot checkpoint"
            );
        }

        for buffered in deltas {
            let delta_id = buffered.id;
            let has_events = buffered.events.is_some();
            let is_covered_by_checkpoint = covered_delta_ids.contains(&delta_id);

            match replay_buffered_delta(ReplayBufferedDeltaInput {
                context_client: self.context_client.clone(),
                node_client: self.node_client.clone(),
                node_state: self.node_state.clone(),
                context_id,
                our_identity,
                buffered,
                sync_timeout: self.sync_config.timeout,
                is_covered_by_checkpoint,
            })
            .await
            {
                Ok(applied) => {
                    if applied {
                        info!(
                            %context_id,
                            delta_id = ?delta_id,
                            has_events,
                            "Replayed buffered delta successfully"
                        );
                    } else if is_covered_by_checkpoint {
                        debug!(
                            %context_id,
                            delta_id = ?delta_id,
                            "Buffered delta is ancestor of checkpoint (state covered, handlers executed)"
                        );
                    } else {
                        debug!(
                            %context_id,
                            delta_id = ?delta_id,
                            "Buffered delta went to pending (missing parents)"
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        %context_id,
                        delta_id = ?delta_id,
                        error = %e,
                        "Failed to replay buffered delta"
                    );
                }
            }
        }
    }

    /// Fine-sync from snapshot boundary to catch up to latest state.
    async fn fine_sync_from_boundary(
        &self,
        context_id: ContextId,
        peer_id: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        // Fresh DeltaStore created here must be hydrated once from DB;
        // warm stores are kept current by execute-side incremental
        // notifications.
        let (delta_store, is_new) = {
            let context_client = self.context_client.clone();
            self.state_access.get_or_register_delta_store(
                context_id,
                Box::new(move || {
                    crate::delta_store::DeltaStore::new(
                        [0u8; 32],
                        context_client,
                        context_id,
                        our_identity,
                    )
                }),
            )
        };
        if is_new {
            if let Err(e) = delta_store.load_persisted_deltas().await {
                warn!(
                    ?e,
                    %context_id,
                    "Failed to hydrate freshly-created DeltaStore from DB"
                );
            }
        }

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

    pub async fn handle_opened_stream(&self, peer_id: PeerId, mut stream: Box<Stream>) {
        loop {
            match self
                .internal_handle_opened_stream(peer_id, &mut stream)
                .await
            {
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

    async fn internal_handle_opened_stream(
        &self,
        peer_id: PeerId,
        stream: &mut Stream,
    ) -> eyre::Result<Option<()>> {
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
            unexpected @ (StreamMessage::Message { .. }
            | StreamMessage::OpaqueError
            | StreamMessage::NotMaterialized) => {
                bail!("expected initialization handshake, got {:?}", unexpected)
            }
        };

        if let InitPayload::NamespaceBackfillRequest {
            namespace_id,
            delta_ids,
        } = &payload
        {
            self.handle_namespace_backfill_request(*namespace_id, delta_ids, stream, nonce)
                .await?;
            return Ok(Some(()));
        }

        if let InitPayload::NamespaceJoinRequest {
            namespace_id,
            ref invitation_bytes,
            joiner_public_key,
        } = &payload
        {
            self.handle_namespace_join_request(
                *namespace_id,
                invitation_bytes,
                *joiner_public_key,
                stream,
                nonce,
            )
            .await?;
            return Ok(Some(()));
        }

        if let InitPayload::OpenSubgroupJoinRequest {
            namespace_id,
            subgroup_id,
            joiner_public_key,
        } = &payload
        {
            self.handle_open_subgroup_join_request(
                *namespace_id,
                *subgroup_id,
                *joiner_public_key,
                stream,
                nonce,
            )
            .await?;
            return Ok(Some(()));
        }

        let context = match self.context_client.get_context(&context_id)? {
            Some(ctx) => ctx,
            None => {
                // Race window: the dialer can trigger context-level sync as
                // a cascade of namespace-topic subscription
                // (`subscriptions.rs::handle_subscribed` → `sync_group` /
                // `broadcast_group_local_state`) before this node's local
                // state has caught up. Two distinct sub-races can leave
                // `get_context` returning `None` for a legitimate inbound:
                //
                //   (1) Namespace governance op `ContextRegistered` has
                //       not yet been processed locally —
                //       `get_group_for_context` returns `None`. This is
                //       the cold-start gossipsub-mesh case (#2122/#2236
                //       residuals tracked in #2356): the namespace topic
                //       takes one or more heartbeats to form a mesh, so
                //       the governance op landing on `peer A` can lag the
                //       context-level sync stream from `peer A` reaching
                //       us by several seconds.
                //   (2) `ContextRegistered` is applied — group binding
                //       exists, dialer is a verified namespace member —
                //       but `join_context` has not yet materialised the
                //       context entry. The original race shape covered
                //       by this branch.
                //
                // Both resolve once the namespace governance DAG settles
                // and `join_context` runs to completion locally. Poll
                // for both in a single shared-deadline loop instead of
                // short-circuiting on (1).
                let store = self.context_client.datastore();

                // Poll cadence matches `FALLBACK_POLL` in
                // `handlers/join_context.rs`. The 10 s budget covers
                // both the (~5 s) `join_context` materialisation gap
                // observed in the `bdc61af` smoke-regression artefact
                // and the additional (~5 s) cold-start namespace-mesh
                // `ContextRegistered` propagation gap observed in
                // mero-drive E2E run 25882151397 (logged in #2356).
                // Streams that don't resolve within the window fall
                // through to the same OpaqueError as before — the
                // dialer retries on its next sync interval.
                const MATERIALIZATION_WINDOW: time::Duration = time::Duration::from_secs(10);
                const MATERIALIZATION_POLL: time::Duration = time::Duration::from_millis(200);

                let deadline = Instant::now() + MATERIALIZATION_WINDOW;
                let mut dialer_verified = false;
                let mut materialised: Option<_> = None;

                loop {
                    if !dialer_verified {
                        if let Some(group_id) =
                            calimero_context::group_store::get_group_for_context(
                                store,
                                &context_id,
                            )?
                        {
                            if MembershipRepository::new(store)
                                .is_member(&group_id, &their_identity)?
                            {
                                dialer_verified = true;
                            }
                        }
                    }

                    if dialer_verified {
                        if let Some(ctx) = self.context_client.get_context(&context_id)? {
                            materialised = Some(ctx);
                            break;
                        }
                    }

                    if Instant::now() >= deadline {
                        break;
                    }
                    time::sleep(MATERIALIZATION_POLL).await;
                }

                if !dialer_verified {
                    // Genuinely unknown context (or cross-namespace stream
                    // leak per #2198), or namespace governance op never
                    // landed within the window. Close cleanly so unrelated
                    // sync activity is unaffected.
                    warn!(
                        %context_id,
                        ?their_identity,
                        "inbound stream for unknown context, closing cleanly"
                    );

                    if let Err(err) = self.send(stream, &StreamMessage::OpaqueError, None).await {
                        error!(%err, %context_id, "failed to send OpaqueError for unknown context");
                    }

                    return Ok(None);
                }

                match materialised {
                    Some(ctx) => {
                        debug!(
                            %context_id,
                            ?their_identity,
                            "context materialised during join race window, proceeding with inbound sync"
                        );
                        ctx
                    }
                    None => {
                        // #2422 Option 4: send the typed
                        // `NotMaterialized` instead of a bare
                        // `OpaqueError`. The dialer was a verified
                        // group member, just hasn't materialised
                        // this context locally (auto-follow opt-out,
                        // pending JoinContext, etc.). The initiator
                        // classifies `NotMaterialized` as benign in
                        // `manager/mod.rs::run_interval_sync_once`
                        // and skips the `on_failure()` /
                        // exponential-backoff path. Keeps long-lived
                        // sync state healthy even when peer
                        // selection picks a non-following peer.
                        debug!(
                            %context_id,
                            ?their_identity,
                            "context not materialised within join race window — sending NotMaterialized"
                        );
                        if let Err(err) = self
                            .send(stream, &StreamMessage::NotMaterialized, None)
                            .await
                        {
                            error!(
                                %err,
                                %context_id,
                                "failed to send NotMaterialized for non-materialised context"
                            );
                        }
                        return Ok(None);
                    }
                }
            }
        };

        let mut _updated = None;

        // Issue #2256: also accept inheritance-eligible parent members
        // for sync auth. `has_member` only knows direct context-membership
        // and direct group-membership; the parent-walk for `Open` subgroups
        // lives in `calimero-context::group_store`, which we have access
        // to here at the node layer.
        let is_inherited_member = || -> eyre::Result<bool> {
            let store = self.context_client.datastore();
            let Some(group_id) =
                calimero_context::group_store::get_group_for_context(store, &context_id)?
            else {
                return Ok(false);
            };
            MembershipRepository::new(store).is_member(&group_id, &their_identity)
        };

        if !self
            .context_client
            .has_member(&context_id, &their_identity)?
            && !is_inherited_member()?
        {
            _updated = Some(
                self.context_client
                    .sync_context_config(context_id, None)
                    .await?,
            );

            if !self
                .context_client
                .has_member(&context_id, &their_identity)?
                && !is_inherited_member()?
            {
                // The peer may have just published MemberAdded for themselves
                // (or their side of the governance DAG is ahead of ours) and
                // gossipsub hasn't delivered it yet. Instead of waiting and
                // hoping the gossip arrives, ask this peer directly for the
                // current namespace governance state on a separate stream —
                // it's the fastest path out of the "unknown member" state and
                // avoids a 30 s stall waiting for `NamespaceStateHeartbeat`.
                //
                // Fire-and-forget governance propagation (issue #2237) is the
                // underlying bug; this is a narrower mitigation in the
                // responder path that converts the terminal close into an
                // active catch-up request.
                self.request_governance_catchup_from_peer(peer_id, &context_id, &their_identity)
                    .await;

                if !self
                    .context_client
                    .has_member(&context_id, &their_identity)?
                    && !is_inherited_member()?
                {
                    // Catch-up didn't resolve it (peer returned nothing, peer
                    // also doesn't know, or the op chain isn't valid locally).
                    // Close gracefully — the initiator retries on their next
                    // sync interval. Demoted from warn to debug because this
                    // is expected during mesh formation and would otherwise
                    // spam logs on every cold join.
                    debug!(
                        %context_id,
                        %their_identity,
                        "unknown context member after namespace backfill request, closing stream"
                    );
                    return Ok(Some(()));
                }
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

        // Inbound sync-gate (mirror of the outbound gate in
        // `initiate_sync_inner`): if an application upgrade is pending on
        // this context, decline to SERVE context state. An ahead peer
        // (already migrated, so its own outbound gate doesn't fire) could
        // otherwise pull our pre-upgrade state as the initiator and adopt
        // it over its newer migrated state. Only state-reconciliation
        // requests are gated — BlobShare (target-app bytecode),
        // governance/join/backfill payloads are left open because this
        // node needs them to complete its OWN lazy migration. The
        // initiator treats `NotMaterialized` as benign and retries; once
        // this node self-migrates on next access the gate lifts and it
        // serves normally. See `pending_upgrade_target`.
        if matches!(
            &payload,
            InitPayload::DeltaRequest { .. }
                | InitPayload::DagHeadsRequest { .. }
                | InitPayload::SnapshotBoundaryRequest { .. }
                | InitPayload::SnapshotStreamRequest { .. }
                | InitPayload::TreeNodeRequest { .. }
                | InitPayload::LevelWiseRequest { .. }
        ) {
            if let Some(target) = self.pending_upgrade_target(&context_id) {
                info!(
                    %context_id,
                    ?their_identity,
                    target_app = %target,
                    "Declining inbound context-state sync: application upgrade pending (gate)"
                );
                if let Err(err) = self
                    .send(stream, &StreamMessage::NotMaterialized, None)
                    .await
                {
                    error!(%err, %context_id, "failed to send NotMaterialized for upgrade-gated sync");
                }
                return Ok(Some(()));
            }
        }

        match payload {
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
            InitPayload::TreeNodeRequest {
                context_id: requested_context_id,
                node_id,
                max_depth,
            } => {
                // Handle tree node request from peer (HashComparison sync)
                // Wrap stream in transport abstraction
                let mut transport = super::stream::StreamTransport::new(stream);
                self.handle_tree_node_request(
                    requested_context_id,
                    node_id,
                    max_depth,
                    &mut transport,
                    nonce,
                )
                .await?
            }
            InitPayload::LevelWiseRequest {
                context_id: requested_context_id,
                level: first_level,
                parent_ids: first_parent_ids,
            } => {
                // Handle LevelWise request from peer (LevelWise sync responder)
                // Wrap stream in transport abstraction
                let mut transport = super::stream::StreamTransport::new(stream);

                // Get store for protocol execution
                let store = self.context_client.datastore_handle().into_inner();

                // Use the already-resolved our_identity from the top of handle_sync_request
                // (avoids redundant lookup and ensures consistency with other handlers)

                // Build the first request data (already parsed above for routing)
                let first_request = super::level_sync::LevelWiseFirstRequest {
                    level: first_level,
                    parent_ids: first_parent_ids,
                    context_client: Some(self.context_client.clone()),
                };

                // Run the LevelWise responder via the trait method
                use calimero_node_primitives::sync::SyncProtocolExecutor;
                super::level_sync::LevelWiseProtocol::run_responder(
                    &mut transport,
                    &store,
                    requested_context_id,
                    our_identity,
                    first_request,
                )
                .await?
            }
            InitPayload::EntityPush { .. } => {
                // EntityPush is handled within the HashComparison responder loop,
                // not as a top-level stream init. If received here, it means a
                // protocol error — the initiator sent EntityPush outside of a
                // HashComparison session. Log and ignore.
                warn!("Received EntityPush outside of HashComparison session, ignoring");
            }
            InitPayload::EntityDeletePush { .. } => {
                // Like EntityPush, tombstone propagation only occurs inside an
                // established HashComparison session (handled by the responder
                // loop), never as a top-level stream init.
                warn!("Received EntityDeletePush outside of HashComparison session, ignoring");
            }
            InitPayload::NamespaceBackfillRequest { .. } => {
                unreachable!("handled by early return above")
            }
            InitPayload::NamespaceJoinRequest { .. } => {
                unreachable!("handled by early return above")
            }
            InitPayload::OpenSubgroupJoinRequest { .. } => {
                unreachable!("handled by early return above")
            }
        };

        Ok(Some(()))
    }
}

// Reconcile-after-divergence orchestration lives in
// `crate::sync::reconciler`. `SyncManager` exposes a thin forwarder
// (so external callers keep their existing call sites) and implements
// `ReconcileSyncDispatch` so the reconciler can call back through
// `initiate_sync` without a self-referential ownership cycle.

impl SyncManager {
    /// Schedule reconcile-via-anchor for every per-context hash
    /// mismatch in `report`. Called by the namespace governance op
    /// receive handler after `MemberRemoved` / `MemberLeft` apply
    /// reports state-hash divergence from the signed claims.
    ///
    /// See `crate::sync::reconciler::Reconciler::reconcile_after_divergence`
    /// for the orchestration body. This is a forwarder.
    pub async fn reconcile_after_divergence(
        &self,
        report: calimero_context_client::messages::DivergenceReport,
    ) {
        self.reconciler
            .reconcile_after_divergence(self, report)
            .await
    }
}

#[async_trait::async_trait(?Send)]
impl super::reconciler::ReconcileSyncDispatch for SyncManager {
    async fn initiate_sync(
        &self,
        context_id: ContextId,
        peer: PeerId,
    ) -> eyre::Result<(PeerId, SyncProtocol)> {
        SyncManager::initiate_sync(self, context_id, peer).await
    }
}

// Protocol-dispatch back into `SyncManager` for the methods the
// extracted `ProtocolSelector` needs to call. Same `?Send` rationale
// as the reconciler dispatch above.
#[async_trait::async_trait(?Send)]
impl super::protocol_selector::ProtocolDispatch for SyncManager {
    async fn open_stream(&self, peer: PeerId) -> eyre::Result<Stream> {
        self.sync_network
            .open_stream(peer)
            .await
            .wrap_err("open stream")
    }

    async fn request_dag_heads_and_sync(
        &self,
        context_id: ContextId,
        chosen_peer: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<SyncProtocol> {
        SyncManager::request_dag_heads_and_sync(self, context_id, chosen_peer, our_identity, stream)
            .await
    }

    async fn fallback_to_snapshot_sync(
        &self,
        context_id: ContextId,
        our_identity: PublicKey,
        chosen_peer: PeerId,
    ) -> eyre::Result<SyncProtocol> {
        SyncManager::fallback_to_snapshot_sync(self, context_id, our_identity, chosen_peer).await
    }
}

// Driver-dispatch back into `SyncManager` for the cross-actor message
// handlers the extracted `SyncDriver` needs to call. Same `?Send`
// rationale as the prior dispatch impls.
#[async_trait::async_trait(?Send)]
impl super::driver::SyncDriverDispatch for SyncManager {
    async fn sync_namespace_from_peer(&self, namespace_id: [u8; 32]) {
        SyncManager::sync_namespace_from_peer(self, namespace_id, None).await
    }

    async fn initiate_namespace_join(
        &self,
        params: calimero_node_primitives::client::NamespaceJoinParams,
    ) -> eyre::Result<calimero_node_primitives::join_bundle::JoinBundle> {
        SyncManager::initiate_namespace_join(self, params).await
    }

    async fn initiate_open_subgroup_join(
        &self,
        params: calimero_node_primitives::client::OpenSubgroupJoinParams,
    ) -> eyre::Result<Vec<u8>> {
        SyncManager::initiate_open_subgroup_join(self, params).await
    }
}

// `partition_peers_anchor_first` moved to `sync::peers` as Phase 1 of
// `SyncManager` decomposition. Call sites use
// `super::peers::partition_peers_anchor_first`.

impl SyncManager {
    /// Actively request governance catch-up from a specific peer whose
    /// identity we don't yet recognize as a context member.
    ///
    /// Scenario: a peer opens a sync stream to us, but their identity isn't
    /// in our local governance DAG yet because fire-and-forget `MemberAdded`
    /// gossip (issue #2237) hasn't reached us. The legacy path waited 2 s
    /// for gossip and then closed the stream, stalling the initiator for
    /// up to 30 s (`NamespaceStateHeartbeat` cadence). Instead, open a
    /// separate stream back to the peer with `NamespaceBackfillRequest`
    /// (empty `delta_ids` = "send everything you have for this namespace"),
    /// apply every op they return, and let the caller re-check membership.
    ///
    /// Best-effort: any failure (no group resolved, stream open fails,
    /// peer returns no ops, ops fail to apply) is logged at debug and the
    /// caller proceeds to close the stream as before. The real fix is the
    /// three-phase contract in #2237; this is a responder-side bandaid
    /// that turns a 30 s stall into at worst a second round-trip.
    async fn request_governance_catchup_from_peer(
        &self,
        peer_id: PeerId,
        context_id: &ContextId,
        their_identity: &PublicKey,
    ) {
        let store = self.context_client.datastore();
        let namespace_id =
            match calimero_context::group_store::get_group_for_context(store, context_id) {
                Ok(Some(group_id)) => match NamespaceRepository::new(store).resolve(&group_id) {
                    Ok(ns) => ns.to_bytes(),
                    Err(err) => {
                        debug!(
                            %context_id,
                            %their_identity,
                            %err,
                            "failed to resolve namespace for governance catch-up"
                        );
                        return;
                    }
                },
                Ok(None) => {
                    debug!(
                        %context_id,
                        %their_identity,
                        "context not in a group — no namespace to request catch-up from"
                    );
                    return;
                }
                Err(err) => {
                    debug!(
                        %context_id,
                        %their_identity,
                        %err,
                        "failed to resolve group for governance catch-up"
                    );
                    return;
                }
            };

        let mut stream = match self.sync_network.open_stream(peer_id).await {
            Ok(s) => s,
            Err(err) => {
                debug!(
                    %context_id,
                    %their_identity,
                    %peer_id,
                    %err,
                    "failed to open catch-up stream to peer"
                );
                return;
            }
        };

        let msg = StreamMessage::Init {
            context_id: ContextId::from([0u8; 32]),
            party_id: PublicKey::from([0u8; 32]),
            payload: InitPayload::NamespaceBackfillRequest {
                namespace_id,
                delta_ids: Vec::new(),
            },
            next_nonce: rand::thread_rng().gen(),
        };

        if let Err(err) = super::stream::send(&mut stream, &msg, None).await {
            debug!(
                %context_id,
                %their_identity,
                %peer_id,
                %err,
                "failed to send NamespaceBackfillRequest during catch-up"
            );
            return;
        }

        let response = match super::stream::recv(&mut stream, None, self.sync_config.timeout).await
        {
            Ok(Some(StreamMessage::Message {
                payload: MessagePayload::NamespaceBackfillResponse { deltas },
                ..
            })) => deltas,
            Ok(_) => {
                debug!(
                    %context_id,
                    %their_identity,
                    %peer_id,
                    "unexpected response to NamespaceBackfillRequest during catch-up"
                );
                return;
            }
            Err(err) => {
                debug!(
                    %context_id,
                    %their_identity,
                    %peer_id,
                    %err,
                    "catch-up NamespaceBackfillRequest timed out or failed"
                );
                return;
            }
        };

        if response.is_empty() {
            debug!(
                %context_id,
                %their_identity,
                %peer_id,
                "peer returned no namespace ops for catch-up"
            );
            return;
        }

        use calimero_context_client::messages::NamespaceApplyOutcome;
        let ops_count = response.len();
        let mut applied = 0usize;
        let mut newly_applied = 0usize;
        for (_delta_id, op_bytes) in response {
            let op = match borsh::from_slice::<
                calimero_context_client::local_governance::SignedNamespaceOp,
            >(&op_bytes)
            {
                Ok(o) => o,
                Err(err) => {
                    debug!(
                        %context_id,
                        %their_identity,
                        %err,
                        "failed to decode catch-up op"
                    );
                    continue;
                }
            };
            match self.context_client.apply_signed_namespace_op(op).await {
                Ok(NamespaceApplyOutcome::Applied { .. }) => {
                    applied += 1;
                    newly_applied += 1;
                }
                Ok(_) => {
                    applied += 1;
                }
                Err(err) => {
                    debug!(
                        %context_id,
                        %their_identity,
                        %err,
                        "failed to apply catch-up op"
                    );
                    continue;
                }
            }
        }

        // Single FSM notification after the batch when we actually
        // advanced the local applied_through. `Pending` (parents missing)
        // and `Duplicate` outcomes are no-progress from the FSM's POV,
        // so we skip the mailbox hop in those cases. Mirrors the gate
        // used at `network_event/namespace.rs:120`.
        if newly_applied > 0 {
            self.node_client.notify_namespace_op_applied(namespace_id);
        }

        // Parity with the gossip apply path: a governance op we just learned
        // may unblock a state delta buffered as `Unknown`. Run whenever this
        // catch-up returned ops, not only on a fresh apply — see
        // `drain_governance_pending_after_sync`.
        if ops_count > 0 {
            self.drain_governance_pending_after_sync().await;
        }

        debug!(
            %context_id,
            %their_identity,
            %peer_id,
            ops_received = ops_count,
            ops_applied = applied,
            "governance catch-up complete"
        );
    }

    /// Release any state deltas parked in the governance-pending buffer after
    /// a governance-sync path applied (or re-confirmed) ops.
    ///
    /// The gossip apply path (`network_event/namespace.rs`) already drains the
    /// governance-pending buffer when a namespace op applies, but the
    /// **sync/backfill** apply paths here did not — a parity gap. A late
    /// joiner's first post-join state delta is buffered as
    /// `MembershipStatus::Unknown` until the local node learns the joiner's
    /// membership op; when that op arrives via sync (beacon-triggered
    /// governance sync or catch-up backfill) rather than gossip, nothing
    /// re-evaluated the buffer, so the delta sat there forever and the two
    /// nodes' context root hashes never reconverged.
    ///
    /// Deliberately *not* gated on a fresh `Applied` outcome: the awaited op
    /// may already be present locally (e.g. deduplicated on read, #2327) yet
    /// no drain has ever fired for it. Re-evaluating membership is the correct
    /// trigger, and the call is cheap — `drain_all_governance_pending` returns
    /// immediately when no context holds buffered deltas.
    async fn drain_governance_pending_after_sync(&self) {
        let drain_input = crate::handlers::state_delta::StateDeltaContext {
            node_clients: crate::state::NodeClients {
                context: self.context_client.clone(),
                node: self.node_client.clone(),
            },
            node_state: self.node_state.clone(),
            network_client: self.network_client.clone(),
            sync_timeout: self.sync_config.timeout,
        };
        crate::handlers::state_delta::drain_all_governance_pending(&drain_input).await;
    }

    /// #2625: when `context_id` has state deltas parked in the
    /// governance-pending buffer, proactively pull its namespace governance
    /// DAG so the missing governance op lands and the buffered deltas drain.
    ///
    /// This closes the gap left by #2589: that fix drains the buffer *when a
    /// governance op is applied* via sync, but here the op is never delivered
    /// to us at all. The only local record that the op exists is the buffered
    /// delta's `governance_position`; our governance DAG has no missing-parent
    /// entry for it, so `resolve_namespace_pending` (which gates on
    /// `namespace_has_pending`) is a no-op and never requests it. Actively
    /// pulling the namespace DAG is what fetches the op; `sync_namespace_from_peer`
    /// then calls `drain_governance_pending_after_sync` once any ops arrive.
    ///
    /// Peer selection matters: the missing op is almost always an *encrypted
    /// group op*, and only a group **member** stores it as a full
    /// `StoredNamespaceEntry::Signed` (a non-member namespace subscriber holds
    /// only the `Opaque` skeleton and serves nothing for it). So we target the
    /// peers that actually delivered the stuck deltas first — they satisfied
    /// the delta's governance position at send time, hence hold the `Signed`
    /// op — and only fall back to an arbitrary mesh peer if that didn't drain
    /// the buffer (e.g. the delta was relayed by a non-member).
    ///
    /// Gated on a non-empty buffer (a cheap `DashMap` length read), so the
    /// steady-state cost on every interval tick is one map lookup.
    async fn backfill_governance_for_pending_deltas(&self, context_id: ContextId) {
        if !should_backfill_governance(self.node_state.governance_pending_len(&context_id)) {
            return;
        }
        let store = self.context_client.datastore_handle().into_inner();
        let Some(namespace_id) = resolve_namespace_id(&store, &context_id) else {
            debug!(
                %context_id,
                "governance-pending backfill: could not resolve namespace id; skipping (#2625)"
            );
            return;
        };
        drop(store);
        debug!(
            %context_id,
            namespace_id = %hex::encode(namespace_id),
            pending = self.node_state.governance_pending_len(&context_id),
            "governance-pending backfill: pulling namespace governance DAG to release buffered deltas (#2625)"
        );

        // Prefer the peers that delivered the stuck deltas (likely group
        // members holding the full `Signed` op). Stop as soon as the buffer
        // drains so we don't open redundant streams.
        for peer in self.node_state.governance_pending_source_peers(&context_id) {
            if !should_backfill_governance(self.node_state.governance_pending_len(&context_id)) {
                return;
            }
            self.sync_namespace_from_peer(namespace_id, Some(peer))
                .await;
        }

        // Fallback: a non-member relay may have delivered the delta, so its
        // source peer couldn't serve the op. Try the namespace mesh — but
        // anyone can subscribe to the `ns/<id>` topic without being a member,
        // so prefer trusted ANCHORS (peers we've observed signing applied
        // messages with an Owner/Admin/ReadOnlyTee identity) over arbitrary
        // subscribers, exactly like the regular context-sync partner picker.
        //
        // This is a *liveness* defense, not a safety one: a malicious or
        // non-member subscriber cannot corrupt our governance state — every
        // op is signature-verified in `apply_signed_op` before any mutation,
        // is content-hash idempotent, and is nonce/DAG-ordered. The worst a
        // bad peer can do is serve nothing or stale ops; anchor-first ordering
        // just avoids wasting backfill rounds on such peers.
        if should_backfill_governance(self.node_state.governance_pending_len(&context_id)) {
            let topic =
                libp2p::gossipsub::TopicHash::from_raw(format!("ns/{}", hex::encode(namespace_id)));
            let mut peers = self.sync_network.subscribed_peers(topic).await;
            let _anchor_count = super::peers::partition_peers_anchor_first(
                &mut peers,
                &*self.state_access,
                &self.anchor_identities_for_context(&context_id),
            );
            for peer in peers {
                if !should_backfill_governance(self.node_state.governance_pending_len(&context_id))
                {
                    break;
                }
                self.sync_namespace_from_peer(namespace_id, Some(peer))
                    .await;
            }
        }
    }

    /// Handle a namespace backfill request: look up full `SignedNamespaceOp`
    /// payloads for the requested delta IDs and send them back.
    ///
    /// We scan the namespace governance op store for matching delta IDs.
    /// For each requested delta, if we have the full op (stored when we were
    /// a member at apply time), we include it in the response.
    async fn handle_namespace_backfill_request(
        &self,
        namespace_id: [u8; 32],
        delta_ids: &[[u8; 32]],
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        let store = self.context_client.datastore_handle().into_inner();
        let handle = store.handle();
        let mut found = Vec::new();

        /// Maximum ops returned in a single backfill response to prevent
        /// memory exhaustion from large namespace governance DAGs.
        const MAX_BACKFILL_OPS: usize = 500;

        if delta_ids.is_empty() {
            // Empty request = "give me everything for this namespace".
            let start = calimero_store::key::NamespaceGovOp::new(namespace_id, [0u8; 32]);
            let mut iter = handle.iter::<calimero_store::key::NamespaceGovOp>()?;
            let first = iter.seek(start).transpose();

            for entry in first.into_iter().chain(iter.keys()) {
                let key = match entry {
                    Ok(k) => k,
                    Err(_) => break,
                };
                if key.namespace_id() != namespace_id {
                    break;
                }
                if let Ok(Some(value)) = handle.get(&key) {
                    if let Some(signed_bytes) =
                        crate::sync::helpers::extract_signed_op_bytes(&value.skeleton_bytes)
                    {
                        found.push((key.delta_id(), signed_bytes));
                        if found.len() >= MAX_BACKFILL_OPS {
                            break;
                        }
                    }
                }
            }
        } else {
            for delta_id in delta_ids.iter().take(MAX_BACKFILL_OPS) {
                let key = calimero_store::key::NamespaceGovOp::new(namespace_id, *delta_id);
                if let Ok(Some(value)) = handle.get(&key) {
                    if let Some(signed_bytes) =
                        crate::sync::helpers::extract_signed_op_bytes(&value.skeleton_bytes)
                    {
                        found.push((*delta_id, signed_bytes));
                    }
                }
            }
        }

        let msg = StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::NamespaceBackfillResponse { deltas: found },
            next_nonce: nonce,
        };
        super::stream::send(stream, &msg, None).await?;
        Ok(())
    }

    /// Handle an incoming NamespaceJoinRequest on the responder side.
    ///
    /// Validates the invitation, wraps the group key for the joiner,
    /// enumerates contexts, and collects governance ops.
    async fn handle_namespace_join_request(
        &self,
        namespace_id: [u8; 32],
        invitation_bytes: &[u8],
        joiner_public_key: PublicKey,
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        use calimero_context::group_store::enumerate_group_contexts;
        use calimero_context_config::types::ContextGroupId;
        use calimero_context_config::types::SignedGroupOpenInvitation;

        let _invitation: SignedGroupOpenInvitation = match borsh::from_slice(invitation_bytes) {
            Ok(inv) => inv,
            Err(err) => {
                let msg = StreamMessage::Message {
                    sequence_id: 0,
                    payload: MessagePayload::NamespaceJoinRejected {
                        reason: format!("invalid invitation: {err}"),
                    },
                    next_nonce: nonce,
                };
                super::stream::send(stream, &msg, None).await?;
                return Ok(());
            }
        };

        let group_id = ContextGroupId::from(namespace_id);
        let store = self.context_client.datastore_handle().into_inner();

        let meta = match MetaRepository::new(&store).load(&group_id)? {
            Some(m) => m,
            None => {
                let msg = StreamMessage::Message {
                    sequence_id: 0,
                    payload: MessagePayload::NamespaceJoinRejected {
                        reason: "group not found".to_owned(),
                    },
                    next_nonce: nonce,
                };
                super::stream::send(stream, &msg, None).await?;
                return Ok(());
            }
        };

        let key_envelope_bytes = match GroupKeyring::new(&store, group_id).load_current_key()? {
            Some((_key_id, group_key)) => {
                let ns_identity =
                    NamespaceRepository::new(&store).resolve_identity_record(&group_id)?;
                match ns_identity {
                    Some(record) => {
                        let sender_sk =
                            calimero_primitives::identity::PrivateKey::from(record.private_key);
                        match GroupKeyring::wrap_for_member(
                            &sender_sk,
                            &joiner_public_key,
                            &group_key,
                        ) {
                            Ok(envelope) => borsh::to_vec(&envelope).unwrap_or_default(),
                            Err(err) => {
                                warn!(
                                    namespace_id = %hex::encode(namespace_id),
                                    %err,
                                    "failed to wrap group key for joiner"
                                );
                                Vec::new()
                            }
                        }
                    }
                    None => {
                        warn!(
                            namespace_id = %hex::encode(namespace_id),
                            "no namespace identity found, cannot wrap key"
                        );
                        Vec::new()
                    }
                }
            }
            None => Vec::new(),
        };

        // Pre-register the joiner as a group member and write ContextIdentity
        // entries so that when the joiner opens a sync stream, this node's
        // membership check (has_member) passes immediately.
        if let Err(e) = MembershipRepository::new(&store).add_member(
            &group_id,
            &joiner_public_key,
            calimero_primitives::context::GroupMemberRole::Member,
        ) {
            warn!(%e, "failed to pre-register joiner as group member");
        }

        let context_ids = enumerate_group_contexts(&store, &group_id, 0, usize::MAX)?;
        let application_id: [u8; 32] = *meta.target_application_id.as_ref();

        for ctx_id in &context_ids {
            let ci_key = calimero_store::key::ContextIdentity::new(*ctx_id, joiner_public_key);
            let mut handle = store.handle();
            if !handle.has(&ci_key).unwrap_or(false) {
                let _ = handle.put(
                    &ci_key,
                    &calimero_store::types::ContextIdentity {
                        private_key: None,
                        sender_key: None,
                    },
                );
            }
        }

        let governance_ops = self.collect_namespace_governance_ops(namespace_id)?;

        // Issue #2256: the namespace's default-capabilities value travels
        // with the bundle so the joiner doesn't need to fall back to a
        // hard-coded constant. Read whatever the responder currently
        // believes (already reflects any admin-issued
        // `DefaultCapabilitiesSet` ops because the local store is
        // updated as those ops apply). `unwrap_or(0)` matches the
        // pre-existing semantics for "default key absent."
        let default_capabilities = CapabilitiesRepository::new(&store)
            .default_capabilities(&group_id)?
            .unwrap_or(0);

        debug!(
            namespace_id = %hex::encode(namespace_id),
            has_key = !key_envelope_bytes.is_empty(),
            context_count = context_ids.len(),
            app_id = %hex::encode(application_id),
            governance_ops_count = governance_ops.len(),
            default_capabilities,
            "Sending NamespaceJoinResponse"
        );

        let msg = StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::NamespaceJoinResponse {
                key_envelope_bytes,
                context_ids,
                application_id,
                governance_ops,
                default_capabilities,
            },
            next_nonce: nonce,
        };
        super::stream::send(stream, &msg, None).await?;
        Ok(())
    }

    /// Handle an incoming `OpenSubgroupJoinRequest` (issue #2357) on the
    /// responder side. Validates that the joiner has
    /// `MembershipPath::Inherited` to the requested subgroup, wraps the
    /// local subgroup key for the joiner via ECDH, and replies with the
    /// envelope. Mirrors `handle_namespace_join_request` for the
    /// inherited self-join path.
    async fn handle_open_subgroup_join_request(
        &self,
        namespace_id: [u8; 32],
        subgroup_id: [u8; 32],
        joiner_public_key: PublicKey,
        stream: &mut Stream,
        nonce: Nonce,
    ) -> eyre::Result<()> {
        use calimero_context::group_store::MembershipPath;
        use calimero_context_config::types::ContextGroupId;

        let subgroup_gid = ContextGroupId::from(subgroup_id);
        let store = self.context_client.datastore_handle().into_inner();

        // Cross-namespace pin: the requested subgroup must belong to the
        // namespace the joiner named, otherwise an attacker on namespace
        // A could elicit a key for a subgroup of namespace B.
        match NamespaceRepository::new(&store).resolve(&subgroup_gid) {
            Ok(ns) if ns.to_bytes() == namespace_id => {}
            Ok(other_ns) => {
                let msg = StreamMessage::Message {
                    sequence_id: 0,
                    payload: MessagePayload::OpenSubgroupJoinRejected {
                        reason: format!(
                            "subgroup belongs to namespace {} not {}",
                            hex::encode(other_ns.to_bytes()),
                            hex::encode(namespace_id),
                        ),
                    },
                    next_nonce: nonce,
                };
                super::stream::send(stream, &msg, None).await?;
                return Ok(());
            }
            Err(err) => {
                let msg = StreamMessage::Message {
                    sequence_id: 0,
                    payload: MessagePayload::OpenSubgroupJoinRejected {
                        reason: format!("resolve namespace: {err}"),
                    },
                    next_nonce: nonce,
                };
                super::stream::send(stream, &msg, None).await?;
                return Ok(());
            }
        }

        if MetaRepository::new(&store).load(&subgroup_gid)?.is_none() {
            let msg = StreamMessage::Message {
                sequence_id: 0,
                payload: MessagePayload::OpenSubgroupJoinRejected {
                    reason: "subgroup not found locally".to_owned(),
                },
                next_nonce: nonce,
            };
            super::stream::send(stream, &msg, None).await?;
            return Ok(());
        }

        // Authorisation check: the joiner must reach the subgroup via the
        // Open-chain inheritance walk. `MembershipPath::Inherited`
        // implies every intermediate ancestor was Open (see
        // `membership.rs:267`), so this is the proof of authorisation.
        match MembershipRepository::new(&store).check_path(&subgroup_gid, &joiner_public_key)? {
            MembershipPath::Inherited { .. } | MembershipPath::Direct => {}
            MembershipPath::None => {
                let msg = StreamMessage::Message {
                    sequence_id: 0,
                    payload: MessagePayload::OpenSubgroupJoinRejected {
                        reason: "joiner has no membership path to subgroup".to_owned(),
                    },
                    next_nonce: nonce,
                };
                super::stream::send(stream, &msg, None).await?;
                return Ok(());
            }
        }

        let key_envelope_bytes = match GroupKeyring::new(&store, subgroup_gid).load_current_key()? {
            Some((_key_id, group_key)) => {
                let ns_gid = ContextGroupId::from(namespace_id);
                match NamespaceRepository::new(&store).resolve_identity_record(&ns_gid)? {
                    Some(record) => {
                        let sender_sk =
                            calimero_primitives::identity::PrivateKey::from(record.private_key);
                        match GroupKeyring::wrap_for_member(
                            &sender_sk,
                            &joiner_public_key,
                            &group_key,
                        ) {
                            Ok(envelope) => borsh::to_vec(&envelope).unwrap_or_default(),
                            Err(err) => {
                                warn!(
                                    namespace_id = %hex::encode(namespace_id),
                                    subgroup_id = %hex::encode(subgroup_id),
                                    %err,
                                    "failed to wrap subgroup key for joiner"
                                );
                                Vec::new()
                            }
                        }
                    }
                    None => {
                        warn!(
                            namespace_id = %hex::encode(namespace_id),
                            "no namespace identity, cannot wrap subgroup key"
                        );
                        Vec::new()
                    }
                }
            }
            None => Vec::new(),
        };

        debug!(
            namespace_id = %hex::encode(namespace_id),
            subgroup_id = %hex::encode(subgroup_id),
            has_key = !key_envelope_bytes.is_empty(),
            "Sending OpenSubgroupJoinResponse"
        );

        let msg = StreamMessage::Message {
            sequence_id: 0,
            payload: MessagePayload::OpenSubgroupJoinResponse { key_envelope_bytes },
            next_nonce: nonce,
        };
        super::stream::send(stream, &msg, None).await?;
        Ok(())
    }

    /// Initiator side for `request_open_subgroup_join`. Picks a mesh peer
    /// on the namespace topic, opens a stream, sends the request, and
    /// returns the wrapped key envelope. Same peer-discovery retry loop
    /// as `initiate_namespace_join`.
    async fn initiate_open_subgroup_join(
        &self,
        params: OpenSubgroupJoinParams,
    ) -> eyre::Result<Vec<u8>> {
        let topic = libp2p::gossipsub::TopicHash::from_raw(format!(
            "ns/{}",
            hex::encode(params.namespace_id)
        ));

        let mut peers = Vec::new();
        for attempt in 1..=super::config::DEFAULT_MESH_RETRIES_UNINITIALIZED {
            peers = self.sync_network.subscribed_peers(topic.clone()).await;
            if !peers.is_empty() {
                break;
            }
            if attempt < super::config::DEFAULT_MESH_RETRIES_UNINITIALIZED {
                debug!(
                    namespace_id = %hex::encode(params.namespace_id),
                    subgroup_id = %hex::encode(params.subgroup_id),
                    attempt,
                    "No namespace mesh peers yet for open-subgroup join, retrying..."
                );
                time::sleep(std::time::Duration::from_millis(
                    super::config::DEFAULT_MESH_RETRY_DELAY_MS_UNINITIALIZED,
                ))
                .await;
            }
        }

        if peers.is_empty() {
            eyre::bail!(
                "no mesh peers for namespace {} (open-subgroup join)",
                hex::encode(params.namespace_id)
            );
        }

        // Try every mesh peer, not just the first. Only peers that
        // already hold the subgroup key can serve the request — for an
        // `Open` subgroup that is the creator plus anyone who has
        // already inherited in. A freshly-joined namespace member
        // (which is also on the `ns/<hex>` topic) replies with an empty
        // envelope ("responder did not hold the subgroup key"); picking
        // `peers.first()` would fail the whole join whenever that peer
        // happened to be key-less. Walk the list: return on the first
        // peer that yields a key, skip key-less peers, and remember the
        // last authorization rejection so it surfaces if NO peer
        // accepts (a rejection from one peer can be a stale cold-start
        // view while another peer accepts).
        let mut last_rejection: Option<String> = None;
        let mut keyless_peers = 0usize;
        let mut transport_errors = 0usize;

        for peer in &peers {
            let mut stream = match self.sync_network.open_stream(*peer).await {
                Ok(s) => s,
                Err(e) => {
                    debug!(
                        peer = %peer,
                        subgroup_id = %hex::encode(params.subgroup_id),
                        error = %e,
                        "open-subgroup join: failed to open stream, trying next peer"
                    );
                    transport_errors += 1;
                    continue;
                }
            };

            let msg = StreamMessage::Init {
                context_id: calimero_primitives::context::ContextId::from([0u8; 32]),
                party_id: params.joiner_public_key,
                payload: InitPayload::OpenSubgroupJoinRequest {
                    namespace_id: params.namespace_id,
                    subgroup_id: params.subgroup_id,
                    joiner_public_key: params.joiner_public_key,
                },
                next_nonce: rand::thread_rng().gen(),
            };

            if let Err(e) = super::stream::send(&mut stream, &msg, None).await {
                debug!(
                    peer = %peer,
                    error = %e,
                    "open-subgroup join: send failed, trying next peer"
                );
                transport_errors += 1;
                continue;
            }

            match super::stream::recv(&mut stream, None, self.sync_config.timeout).await {
                Ok(Some(StreamMessage::Message {
                    payload: MessagePayload::OpenSubgroupJoinResponse { key_envelope_bytes },
                    ..
                })) => {
                    if key_envelope_bytes.is_empty() {
                        // Peer is on the namespace topic but doesn't
                        // hold the subgroup key — try the next one.
                        keyless_peers += 1;
                        continue;
                    }
                    return Ok(key_envelope_bytes);
                }
                Ok(Some(StreamMessage::Message {
                    payload: MessagePayload::OpenSubgroupJoinRejected { reason },
                    ..
                })) => {
                    // A rejection may be a stale cold-start view on this
                    // peer; keep trying others before surfacing it.
                    debug!(
                        peer = %peer,
                        reason = %reason,
                        "open-subgroup join: peer rejected, trying next peer"
                    );
                    last_rejection = Some(reason);
                    continue;
                }
                Ok(other) => {
                    debug!(
                        peer = %peer,
                        "open-subgroup join: unexpected response {:?}, trying next peer",
                        other.as_ref().map(std::mem::discriminant)
                    );
                    transport_errors += 1;
                    continue;
                }
                Err(e) => {
                    debug!(
                        peer = %peer,
                        error = %e,
                        "open-subgroup join: recv failed, trying next peer"
                    );
                    transport_errors += 1;
                    continue;
                }
            }
        }

        // No peer yielded the key. Surface the most informative cause,
        // always including the full per-peer tally so a mixed failure
        // (some peers key-less, one peer rejecting, some transport
        // errors) is fully diagnosable from a single line.
        let tally = format!(
            "{} peer(s): {} key-less, {} transport error(s)",
            peers.len(),
            keyless_peers,
            transport_errors
        );
        if let Some(reason) = last_rejection {
            eyre::bail!(
                "open-subgroup join for {} served by no peer — last rejection: {} [{}]",
                hex::encode(params.subgroup_id),
                reason,
                tally
            );
        }
        eyre::bail!(
            "no mesh peer held the subgroup key for {} [{}]",
            hex::encode(params.subgroup_id),
            tally
        );
    }

    /// Collect all governance ops for a namespace (reused by the join responder).
    ///
    /// Returns bare `SignedNamespaceOp` bytes (not `StoredNamespaceEntry` wrapped)
    /// so recipients can `borsh::from_slice::<SignedNamespaceOp>` directly.
    fn collect_namespace_governance_ops(
        &self,
        namespace_id: [u8; 32],
    ) -> eyre::Result<Vec<Vec<u8>>> {
        let store = self.context_client.datastore_handle().into_inner();
        let handle = store.handle();
        let mut ops = Vec::new();

        let start = calimero_store::key::NamespaceGovOp::new(namespace_id, [0u8; 32]);
        let mut iter = handle.iter::<calimero_store::key::NamespaceGovOp>()?;
        let first = iter.seek(start).transpose();

        for entry in first.into_iter().chain(iter.keys()) {
            let key = match entry {
                Ok(k) => k,
                Err(_) => break,
            };
            if key.namespace_id() != namespace_id {
                break;
            }
            if let Ok(Some(value)) = handle.get(&key) {
                if let Some(bytes) =
                    crate::sync::helpers::extract_signed_op_bytes(&value.skeleton_bytes)
                {
                    ops.push(bytes);
                }
            }
        }

        Ok(ops)
    }

    /// Initiator side: open a stream to a mesh peer and perform the
    /// NamespaceJoinRequest / NamespaceJoinResponse exchange.
    async fn initiate_namespace_join(
        &self,
        params: NamespaceJoinParams,
    ) -> eyre::Result<JoinBundle> {
        // Connect-loop logic (shuffled-peer retry, per-peer timeout,
        // outer deadline) lives in `namespace_join::open_namespace_join_stream`
        // so it can be unit-tested against `MockSyncNetwork` without
        // standing up a full `SyncManager`. See that module for the
        // design rationale (mesh-formation latency, stale-transport
        // fallback, deadline budgeting under large meshes).
        //
        // Outer loop retries the entire connect-and-exchange when the
        // chosen peer returns `NamespaceJoinRejected` or fails the
        // post-open send/recv. A peer can be in the gossipsub mesh
        // and reachable on transport but not yet have processed the
        // namespace governance DAG far enough to serve the join —
        // rejecting that peer must not fail the whole join when
        // another mesh peer is in a position to answer. Mirrors the
        // pattern `initiate_open_subgroup_join` uses for the same
        // mesh-cold-peer race.
        //
        // Rejected peers feed back into `open_namespace_join_stream`
        // via `excluded_peers` so the next round skips them at the
        // connect layer rather than re-opening a transport just to
        // get rejected again.
        let mut rejected_peers: std::collections::HashSet<libp2p::PeerId> =
            std::collections::HashSet::new();
        let mut last_rejection: Option<String> = None;
        let mut last_connect_err: Option<String> = None;
        // Cap on protocol-level retries. The connect loop already
        // handles transport failure across peers; this cap bounds the
        // total post-open exchanges so a small mesh full of stale
        // peers can't deadlock the join indefinitely. Sized to cover
        // typical 1–3 mesh peers plus headroom.
        const MAX_PROTOCOL_RETRIES: usize = 5;

        for protocol_attempt in 1..=MAX_PROTOCOL_RETRIES {
            let (mut stream, peer) = match namespace_join::open_namespace_join_stream(
                &*self.sync_network,
                params.namespace_id,
                self.sync_config.open_stream_timeout,
                super::config::DEFAULT_MESH_RETRIES_UNINITIALIZED,
                std::time::Duration::from_millis(
                    super::config::DEFAULT_MESH_RETRY_DELAY_MS_UNINITIALIZED,
                ),
                &rejected_peers,
            )
            .await
            {
                Ok(opened) => opened,
                Err(open_err) => {
                    if last_rejection.is_none() {
                        // First attempt's connect loop exhausted with
                        // no prior protocol-level success. The
                        // connect loop has its own mesh-retry budget;
                        // re-running it immediately would repeat the
                        // same exhaustion with no state change.
                        // Surface the connect_err directly.
                        return Err(open_err);
                    }
                    // Connect failure *after* at least one peer has
                    // rejected: do not bail. The mesh may surface a
                    // fresh peer on a later protocol attempt that
                    // wasn't visible during this one (mesh-formation
                    // delay, peer just finished processing the
                    // namespace governance DAG, etc.). Record the err
                    // for the exhaustion diagnostic and let the loop
                    // continue.
                    debug!(
                        namespace_id = %hex::encode(params.namespace_id),
                        attempt = protocol_attempt,
                        error = %open_err,
                        "namespace join: connect failed after prior rejection, will retry"
                    );
                    last_connect_err = Some(open_err.to_string());
                    continue;
                }
            };

            let msg = StreamMessage::Init {
                context_id: calimero_primitives::context::ContextId::from([0u8; 32]),
                party_id: params.joiner_public_key,
                payload: InitPayload::NamespaceJoinRequest {
                    namespace_id: params.namespace_id,
                    invitation_bytes: params.invitation_bytes.clone(),
                    joiner_public_key: params.joiner_public_key,
                },
                next_nonce: rand::thread_rng().gen(),
            };

            if let Err(send_err) = super::stream::send(&mut stream, &msg, None).await {
                debug!(
                    namespace_id = %hex::encode(params.namespace_id),
                    %peer,
                    error = %send_err,
                    "namespace join: send failed, marking peer rejected, trying next peer"
                );
                rejected_peers.insert(peer);
                continue;
            }

            match super::stream::recv(&mut stream, None, self.sync_config.timeout).await {
                Ok(Some(StreamMessage::Message {
                    payload:
                        MessagePayload::NamespaceJoinResponse {
                            key_envelope_bytes,
                            context_ids,
                            application_id,
                            governance_ops,
                            default_capabilities,
                        },
                    ..
                })) => {
                    return Ok(JoinBundle {
                        key_envelope_bytes,
                        context_ids,
                        application_id: application_id.into(),
                        governance_ops,
                        default_capabilities,
                    });
                }
                Ok(Some(StreamMessage::Message {
                    payload: MessagePayload::NamespaceJoinRejected { reason },
                    ..
                })) => {
                    debug!(
                        namespace_id = %hex::encode(params.namespace_id),
                        %peer,
                        %reason,
                        attempt = protocol_attempt,
                        "namespace join: peer rejected, trying next peer"
                    );
                    rejected_peers.insert(peer);
                    last_rejection = Some(reason);
                    continue;
                }
                Ok(other) => {
                    let detail = format!(
                        "unexpected response variant: {:?}",
                        other.as_ref().map(|m| std::mem::discriminant(m))
                    );
                    debug!(
                        namespace_id = %hex::encode(params.namespace_id),
                        %peer,
                        %detail,
                        "namespace join: unexpected response, marking peer rejected"
                    );
                    rejected_peers.insert(peer);
                    // Carry the unexpected-response detail into
                    // `last_rejection` so the exhaustion error keeps
                    // diagnostic context if every retry hits this arm.
                    last_rejection = Some(detail);
                    continue;
                }
                Err(recv_err) => {
                    let detail = format!("recv failed: {recv_err}");
                    debug!(
                        namespace_id = %hex::encode(params.namespace_id),
                        %peer,
                        %detail,
                        "namespace join: recv failed, marking peer rejected, trying next peer"
                    );
                    rejected_peers.insert(peer);
                    // Same rationale as the `Ok(other)` arm above —
                    // carry the recv failure into `last_rejection` so
                    // the exhaustion error remains informative.
                    last_rejection = Some(detail);
                    continue;
                }
            }
        }

        eyre::bail!(
            "namespace join exhausted {} protocol attempts (last rejection: {:?}, \
             last connect_err: {:?}, {} peer(s) rejected)",
            MAX_PROTOCOL_RETRIES,
            last_rejection,
            last_connect_err,
            rejected_peers.len()
        )
    }

    /// Pull all namespace governance ops from a peer.
    ///
    /// `peer = Some(p)` targets `p` explicitly; `None` picks the first mesh
    /// peer subscribed to the namespace topic (the legacy behaviour). Callers
    /// that know a group **member** should target it: only members store the
    /// full [`StoredNamespaceEntry::Signed`] op (carrying the encrypted group
    /// payload), so a non-member namespace subscriber holds only the
    /// [`StoredNamespaceEntry::Opaque`] skeleton and `extract_signed_op`
    /// returns `None` for it — backfilling from such a peer yields nothing for
    /// group ops and would never release a governance-pending delta.
    async fn sync_namespace_from_peer(&self, namespace_id: [u8; 32], peer: Option<PeerId>) {
        use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};

        let peer = match peer {
            Some(p) => p,
            None => {
                let topic = libp2p::gossipsub::TopicHash::from_raw(format!(
                    "ns/{}",
                    hex::encode(namespace_id)
                ));
                let peers = self.sync_network.subscribed_peers(topic).await;
                let Some(p) = peers.first().copied() else {
                    debug!(
                        namespace_id = %hex::encode(namespace_id),
                        "no mesh peers for namespace sync"
                    );
                    return;
                };
                p
            }
        };

        let Ok(mut stream) = self.sync_network.open_stream(peer).await else {
            debug!("failed to open stream for namespace sync");
            return;
        };

        let msg = StreamMessage::Init {
            context_id: calimero_primitives::context::ContextId::from([0u8; 32]),
            party_id: calimero_primitives::identity::PublicKey::from([0u8; 32]),
            payload: InitPayload::NamespaceBackfillRequest {
                namespace_id,
                delta_ids: vec![],
            },
            next_nonce: {
                use rand::Rng;
                rand::thread_rng().gen()
            },
        };

        if let Err(err) = super::stream::send(&mut stream, &msg, None).await {
            debug!(%err, "failed to send NamespaceBackfillRequest");
            return;
        }

        match super::stream::recv(&mut stream, None, self.sync_config.timeout).await {
            Ok(Some(StreamMessage::Message {
                payload: MessagePayload::NamespaceBackfillResponse { deltas },
                ..
            })) => {
                let ops_received = deltas.len();
                info!(
                    namespace_id = %hex::encode(namespace_id),
                    ops = ops_received,
                    "received namespace governance ops from peer"
                );
                use calimero_context_client::messages::NamespaceApplyOutcome;
                let mut newly_applied = false;
                // Collect divergence reports surfaced by `MemberRemoved` /
                // `MemberLeft` ops arriving via the namespace-backfill
                // path. Same reasoning as the gossip-receive path: once
                // the DAG marks an op `Applied`, any later gossipsub
                // arrival of the same op becomes `Duplicate` and the
                // apply work — including the post-apply hash check —
                // is skipped. If a `MemberRemoved` op arrives first via
                // backfill and divergence is dropped here, no later
                // path will re-surface it. Fire reconcile after the
                // batch loop so we don't hold `&mut` borrows across an
                // await on `self`.
                let mut pending_divergences: Vec<
                    calimero_context_client::messages::DivergenceReport,
                > = Vec::new();
                for (delta_id, op_bytes) in deltas {
                    match borsh::from_slice::<
                        calimero_context_client::local_governance::SignedNamespaceOp,
                    >(&op_bytes)
                    {
                        Ok(op) => {
                            match self
                                .context_client
                                .apply_signed_namespace_op(op.clone())
                                .await
                            {
                                Err(err) => {
                                    // Capture enough context to diagnose codec/schema
                                    // mismatches (observed as "Unexpected length of
                                    // input" from the inner GroupOp decode when a
                                    // variant's binary layout has drifted). The
                                    // op-type tag + byte-length give us a fingerprint
                                    // without logging potentially sensitive payload.
                                    let op_kind = match &op.op {
                                        calimero_context_client::local_governance::NamespaceOp::Root(r) => {
                                            format!("Root::{r:?}").split('{').next().unwrap_or("Root").trim().to_owned()
                                        }
                                        calimero_context_client::local_governance::NamespaceOp::Group { .. } => {
                                            "Group".to_owned()
                                        }
                                    };
                                    warn!(
                                        namespace_id = %hex::encode(namespace_id),
                                        delta_id = %hex::encode(delta_id),
                                        op_kind = %op_kind,
                                        signer = %op.signer,
                                        nonce = op.nonce,
                                        op_bytes_len = op_bytes.len(),
                                        ?err,
                                        "failed to apply namespace governance op from backfill"
                                    );
                                }
                                Ok(NamespaceApplyOutcome::Applied { divergence }) => {
                                    newly_applied = true;
                                    if let Some(report) = divergence {
                                        pending_divergences.push(report);
                                    }
                                    // Only react to a *newly-applied*
                                    // `MemberJoined`. On `Duplicate`
                                    // (the common case — a backfill
                                    // re-sends the whole DAG every
                                    // round) re-publishing a fresh
                                    // `KeyDelivery` each time would
                                    // grow the namespace governance
                                    // DAG without bound until it hits
                                    // the backfill cap and never
                                    // converges again (#2319).
                                    crate::key_delivery::maybe_publish_key_delivery(
                                        &self.context_client,
                                        &self.node_client,
                                        &op,
                                    )
                                    .await;
                                }
                                Ok(_) => {}
                            }
                        }
                        Err(err) => {
                            warn!(
                                namespace_id = %hex::encode(namespace_id),
                                delta_id = %hex::encode(delta_id),
                                op_bytes_len = op_bytes.len(),
                                op_bytes_prefix = %hex::encode(&op_bytes[..op_bytes.len().min(64)]),
                                %err,
                                "failed to decode namespace governance op from backfill"
                            );
                        }
                    }
                }
                // FSM notify after the batch — gated on at least one
                // `Applied` outcome (Pending/Duplicate are no-progress).
                // See the governance-catch-up notify above for rationale.
                if newly_applied {
                    self.node_client.notify_namespace_op_applied(namespace_id);
                }

                // Route any divergence reports surfaced during the
                // backfill apply loop to the reconcile-via-anchor path.
                // Run sequentially after the batch finishes; we're
                // already in an async method on `&self` so no spawn
                // is needed here (the gossip-receive path uses
                // `actix::spawn` because it runs inside an actor's
                // mailbox slot; this method is invoked by the sync
                // tick which has no such constraint).
                for report in pending_divergences {
                    self.reconcile_after_divergence(report).await;
                }

                // Parity with the gossip apply path: releasing buffered
                // state deltas waiting on a membership op we just backfilled.
                // This is the path the late-joiner reverse-sync hit — the
                // joiner's first post-join write was buffered as `Unknown`
                // and the membership op that unblocks it arrived here, via
                // backfill, never via gossip, so nothing drained the buffer.
                if ops_received > 0 {
                    self.drain_governance_pending_after_sync().await;
                }
            }
            _ => {
                debug!("unexpected response to namespace sync request");
            }
        }
    }
}

/// Pure trigger predicate for the #2625 governance-pending backfill: the
/// interval sync should pull the namespace governance DAG iff the context
/// has at least one delta parked in the governance-pending buffer.
///
/// Extracted as a free function so the trigger condition is unit-testable
/// without standing up a `SyncManager` + network stack — the regression we
/// guard against is silently dropping the trigger (e.g. inverting the
/// comparison), which would let a cross-DAG-buffered delta wedge a context
/// into permanent split-brain again.
const fn should_backfill_governance(pending_len: usize) -> bool {
    pending_len > 0
}

/// Resolve the namespace-root id (bytes) that owns `context_id`, walking from
/// the context's immediate owning group up to the namespace root. Returns
/// `None` for non-group (legacy) contexts whose `ContextGroupRef` is absent,
/// or on a namespace-resolution error.
///
/// Mirrors `ContextClient::get_context_group_id` (reads `ContextGroupRef`)
/// followed by `NamespaceRepository::resolve`, but as a free function over
/// `&Store` so it is unit-testable. Unlike the interval-sync fallback-topic
/// closure it does NOT best-effort fall back to the immediate group id: the
/// #2625 backfill must pull the *correct* namespace DAG, and a wrong id would
/// silently fail to converge rather than fetch the missing governance op.
fn resolve_namespace_id(store: &calimero_store::Store, context_id: &ContextId) -> Option<[u8; 32]> {
    let handle = store.handle();
    let group_id: [u8; 32] = handle
        .get(&calimero_store::key::ContextGroupRef::new(*context_id))
        .ok()??;
    NamespaceRepository::new(store)
        .resolve(&calimero_context_config::types::ContextGroupId::from(
            group_id,
        ))
        .map(|id| id.to_bytes())
        .ok()
}

mod namespace_join;

#[cfg(test)]
mod tests;
