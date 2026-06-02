use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use actix::{Actor, Addr};
use calimero_blobstore::BlobManager as BlobStore;
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_store::Store;
use prometheus_client::metrics::counter::Counter;

use crate::readiness::{ReadinessCache, ReadinessCacheNotify, ReadinessConfig, ReadinessManager};
use crate::sync::SyncManager;
use crate::{NodeClients, NodeManagers, NodeState};

mod startup;

/// Per-(context, peer) record of a *persisting* same-DAG / different-root
/// divergence observed by the hash-heartbeat. Lets the handler tell a
/// transient, self-healing divergence (the common case — a concurrent sync
/// apply mid-flight, logged at WARN) from one that is genuinely stuck across
/// successive heartbeats (escalated to ERROR + an active recovery sync).
///
/// `count` increments only while the SAME (our, their) hash pair recurs — if
/// either hash moves, sync is making progress and the streak resets to 1. The
/// entry is cleared when the pair converges. Touched only synchronously by
/// `handlers::network_event::heartbeat::handle_hash_heartbeat` on the manager
/// actor, so a plain `HashMap` (no lock) suffices.
#[derive(Debug, Clone)]
pub(crate) struct DivergenceMark {
    pub(crate) our_hash: calimero_primitives::hash::Hash,
    pub(crate) their_hash: calimero_primitives::hash::Hash,
    pub(crate) count: u32,
}

/// Main node orchestrator.
///
/// **SRP Applied**: Clear separation of:
/// - `clients`: External service clients (context, node)
/// - `managers`: Service managers (blobstore, sync)
/// - `state`: Mutable runtime state (caches)
#[derive(Debug)]
pub struct NodeManager {
    pub(crate) clients: NodeClients,
    pub(crate) managers: NodeManagers,
    pub(crate) state: NodeState,
    /// Datastore handle. Held on the manager so `setup_readiness_manager`
    /// can hand a clone to [`ReadinessManager`] for namespace-identity
    /// loading during beacon signing (Phase 7.2), and so the receiver-side
    /// `verify_readiness_beacon` (Phase 7.3) can read the namespace
    /// member set when `handle_readiness_beacon` runs on the manager
    /// actor.
    pub(crate) datastore: Store,
    /// Shared per-namespace readiness-beacon cache. The receiver-side
    /// `network_event::readiness::handle_readiness_beacon` (Phase 7.3)
    /// calls `cache.insert(&beacon)` directly without an actor-mailbox
    /// hop — the cache is internally synchronised, so routing through
    /// the `ReadinessManager` would only add latency.
    ///
    /// Held on the manager (not on `NodeClients`) so the cascaded-event
    /// helpers in `handlers/state_delta` can keep their lightweight
    /// `NodeClients` re-construction without having to plumb the cache
    /// through. Those helpers don't read beacons.
    pub(crate) readiness_cache: Arc<ReadinessCache>,
    /// Per-namespace `Notify` registry paired with `readiness_cache`.
    /// The receiver-side beacon handler calls `notify(ns)` after
    /// `cache.insert(&beacon)` so any in-flight
    /// `await_first_fresh_beacon` future wakes immediately.
    pub(crate) readiness_notify: Arc<ReadinessCacheNotify>,
    /// Address of the [`ReadinessManager`] actor. Wired by
    /// `setup_readiness_manager` in [`Actor::started`]; `None` until
    /// then (and during early-startup races where receivers may fire
    /// before the manager is mounted — drop the message in that case).
    pub(crate) readiness_addr: Option<Addr<ReadinessManager>>,
    /// Sender into the dedicated `StateDeltaActor`. The
    /// `BroadcastMessage::StateDelta` arm in
    /// `handlers::network_event` routes jobs here instead of
    /// `ctx.spawn`'ing them on this actor's Arbiter (issue #2299).
    pub(crate) state_delta_tx: crate::state_delta_bridge::StateDeltaSender,
    /// Sender into the dedicated `SyncSessionActor`. The
    /// `StreamOpened` arm in `handlers::stream_opened` routes
    /// inbound sync streams here instead of `ctx.spawn`'ing them on
    /// this actor's Arbiter (issue #2316).
    pub(crate) sync_session_tx: crate::sync_session_bridge::SyncSessionSender,
    /// `sync_root_hash_divergence_detected_total` — incremented by the
    /// hash-heartbeat handler each time it observes a peer with the same
    /// DAG heads but a different storage root hash (#2319). Lets vmagent
    /// alert on divergence rate without grepping logs; with the #2319
    /// determinism fixes this should stay near zero.
    pub(crate) divergence_detected: Counter,
    /// Per-(context, peer) persistence tracker for same-DAG / different-root
    /// divergence (#2319 follow-up). The hash-heartbeat escalates to `error!`
    /// (and an active recovery sync) only after the SAME divergence survives
    /// `DIVERGENCE_PERSIST_THRESHOLD` consecutive heartbeats; a first/changing
    /// observation logs at `warn!`. Keeps a transient mid-sync divergence from
    /// tripping log-scanning CI (`--e2e-mode`) on unrelated work while still
    /// surfacing a genuinely stuck split-brain. See [`DivergenceMark`].
    pub(crate) divergence_streak:
        HashMap<(calimero_primitives::context::ContextId, libp2p::PeerId), DivergenceMark>,
    /// Per-namespace timestamp of the last beacon-*triggered* governance
    /// sync (#2367). Caps beacon-divergence syncs to one per namespace
    /// per `NS_BEACON_SYNC_DEBOUNCE` window — beacons arrive every ~5s
    /// from every Ready peer, so an un-debounced behind-node would fire
    /// a sync per beacon per peer.
    ///
    /// Shared (`Arc<Mutex<_>>`) because the slot is stamped from inside
    /// the spawned divergence-check future, *after* the async DAG read
    /// confirms a sync is genuinely needed — a beacon from an
    /// already-caught-up peer must not burn the budget for a
    /// genuinely-divergent one. The lock is never held across an await.
    /// Touched only by
    /// `handlers::network_event::readiness::handle_readiness_beacon`.
    pub(crate) ns_beacon_sync_debounce: Arc<Mutex<HashMap<[u8; 32], Instant>>>,
}

impl NodeManager {
    #[expect(clippy::too_many_arguments, reason = "wiring-only constructor")]
    pub(crate) fn new(
        blobstore: BlobStore,
        sync_manager: SyncManager,
        context_client: ContextClient,
        node_client: NodeClient,
        datastore: Store,
        state: NodeState,
        state_delta_tx: crate::state_delta_bridge::StateDeltaSender,
        sync_session_tx: crate::sync_session_bridge::SyncSessionSender,
        divergence_detected: Counter,
    ) -> Self {
        Self {
            clients: NodeClients {
                context: context_client,
                node: node_client,
            },
            managers: NodeManagers {
                blobstore,
                sync: sync_manager,
            },
            state,
            datastore,
            readiness_cache: Arc::new(ReadinessCache::default()),
            readiness_notify: Arc::new(ReadinessCacheNotify::default()),
            readiness_addr: None,
            state_delta_tx,
            sync_session_tx,
            divergence_detected,
            divergence_streak: HashMap::new(),
            ns_beacon_sync_debounce: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Actor for NodeManager {
    type Context = actix::Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.setup_startup_subscriptions(ctx);
        self.setup_maintenance_intervals(ctx);
        self.setup_hash_heartbeat_interval(ctx);
        self.setup_readiness_manager(ctx);
    }
}

impl NodeManager {
    /// Mount the [`ReadinessManager`] actor and store its address so
    /// receiver-side handlers can post `ApplyBeaconLocal` /
    /// `EmitOutOfCycleBeacon`. Idempotent — only mounts once per
    /// manager instance.
    pub(crate) fn setup_readiness_manager(&mut self, _ctx: &mut actix::Context<Self>) {
        if self.readiness_addr.is_some() {
            return;
        }
        let manager = ReadinessManager {
            cache: self.readiness_cache.clone(),
            config: ReadinessConfig::default(),
            state_per_namespace: std::collections::HashMap::new(),
            node_client: self.clients.node.clone(),
            datastore: self.datastore.clone(),
            last_probe_response_at: std::collections::HashMap::new(),
        };
        self.readiness_addr = Some(manager.start());
    }
}
