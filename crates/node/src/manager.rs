use std::sync::Arc;

use actix::{Actor, Addr};
use calimero_blobstore::BlobManager as BlobStore;
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_store::Store;

use crate::readiness::{ReadinessCache, ReadinessCacheNotify, ReadinessConfig, ReadinessManager};
use crate::sync::SyncManager;
use crate::{NodeClients, NodeManagers, NodeState};

mod startup;

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
}

impl NodeManager {
    pub(crate) fn new(
        blobstore: BlobStore,
        sync_manager: SyncManager,
        context_client: ContextClient,
        node_client: NodeClient,
        datastore: Store,
        state: NodeState,
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
