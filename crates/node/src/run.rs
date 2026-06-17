//! Node startup and initialization.
//!
//! **Purpose**: Bootstraps the node with all required services and actors.
//! **Main Function**: `start(NodeConfig)` - initializes and runs the node.
use std::collections::BTreeSet;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use actix::Actor;
use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
use calimero_context::config::ContextConfig;
use calimero_context::ContextManager;
use calimero_context_client::client::ContextClient;
use calimero_network::NetworkManager;
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::config::NetworkConfig;
use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
use calimero_server::config::ServerConfig;
use calimero_store::config::StoreConfig;
use calimero_store::db::Database;
use calimero_store::Store;
use calimero_store_encryption::EncryptedDatabase;
use calimero_store_rocksdb::RocksDB;
use calimero_utils_actix::LazyRecipient;
use camino::Utf8PathBuf;
use libp2p::gossipsub::IdentTopic;
use libp2p::identity::Keypair;
use prometheus_client::registry::Registry;
use tokio::sync::{broadcast, mpsc};
use tracing::info;

use crate::arbiter_pool::ArbiterPool;
use crate::dag_compactor::DagCompactor;
use crate::gc::GarbageCollector;
use crate::network_event_channel::{self, NetworkEventChannelConfig};
use crate::network_event_processor::NetworkEventBridge;
use crate::node_metrics::{self, NodeMetrics};
use crate::state_delta_bridge::{start_state_delta_actor, STATE_DELTA_CHANNEL_CAPACITY};
use crate::sync::{PrometheusSyncMetrics, SyncConfig, SyncManager};
use crate::sync_session_bridge::{start_sync_session_actor, SYNC_SESSION_CHANNEL_CAPACITY};
use crate::NodeManager;

pub use calimero_node_primitives::NodeMode;

/// Configuration for specialized node functionality (e.g., read-only nodes).
#[derive(Debug, Clone)]
pub struct SpecializedNodeConfig {
    /// Topic name for specialized node invite discovery messages.
    pub invite_topic: String,
    /// Whether to accept mock TEE attestation (testing only).
    pub accept_mock_tee: bool,
}

#[derive(Debug)]
pub struct NodeConfig {
    pub home: Utf8PathBuf,
    pub identity: Keypair,
    pub network: NetworkConfig,
    pub sync: SyncConfig,
    pub datastore: StoreConfig,
    pub blobstore: BlobStoreConfig,
    pub context: ContextConfig,
    pub server: ServerConfig,
    pub gc_interval_secs: Option<u64>, // Optional GC interval in seconds (default: 12 hours)
    /// DAG compaction settings (issue #2026). Enabled by default.
    pub dag_compaction: calimero_node_primitives::DagCompactionConfig,
    pub mode: NodeMode,
    pub specialized_node: SpecializedNodeConfig,
    /// Resolved per-execution VM resource limits from the `[runtime.limits]`
    /// config section (unset fields fall back to `VMLimits::default`).
    pub vm_limits: calimero_runtime::logic::VMLimits,
}

pub async fn start(config: NodeConfig) -> eyre::Result<()> {
    let mut registry = <Registry>::default();

    let peer_id = config.identity.public().to_peer_id();

    info!("Peer ID: {}", peer_id);

    // Centralised node-level observability families (build-info beacon,
    // NodeState DashMap gauges, blob-cache eviction counters, HTTP request
    // histograms, process resource gauges). Registered into the same
    // `Registry` that all other subsystems write to, then installed as a
    // process-global handle so synchronous recording sites (blob-cache
    // eviction, delta-store apply, HTTP middleware) can record without
    // plumbing.
    let node_metrics = NodeMetrics::new(&mut registry);
    node_metrics.set_build_info(env!("CARGO_PKG_VERSION"), &peer_id.to_string());
    node_metrics::install_global(node_metrics.clone());

    // Sync protocol metric families (`sync_*` — messages_sent, bytes_sent,
    // round_trips, phase_duration_seconds, snapshot_blocked, verification_
    // failures, buffer_drops, …). The recording sites live behind the
    // `SyncMetricsCollector` trait inside the sync module; the registry
    // entries surface them on /metrics so operators can chart whichever
    // recording sites are wired (and so empty families self-document the
    // schema for future wiring).
    let sync_metrics: Arc<dyn crate::sync::SyncMetricsCollector> =
        Arc::new(PrometheusSyncMetrics::new(&mut registry));

    // Open datastore with optional encryption
    let datastore = if let Some(ref key) = config.datastore.encryption_key {
        info!("Opening encrypted datastore");
        let inner_db = RocksDB::open(&config.datastore)?;
        let encrypted_db = EncryptedDatabase::wrap(inner_db, key.clone())?;
        Store::new(std::sync::Arc::new(encrypted_db))
    } else {
        Store::open::<RocksDB>(&config.datastore)?
    };

    let blob_store = BlobStore::new(datastore.clone(), FileSystem::new(&config.blobstore).await?);
    let blob_manager = BlobManager::new(blob_store.clone());

    let node_recipient = LazyRecipient::new();
    let network_recipient = LazyRecipient::new();
    let context_recipient = LazyRecipient::new();

    // Create dedicated network event channel for reliable message delivery
    // This replaces LazyRecipient<NetworkEvent> to avoid cross-arbiter message loss
    let channel_config = NetworkEventChannelConfig {
        channel_size: 1000,     // Configurable, handles burst patterns
        warning_threshold: 0.8, // Log warning at 80% capacity
        stats_log_interval: Duration::from_secs(30),
    };
    let (network_event_sender, network_event_receiver) =
        network_event_channel::channel(channel_config, &mut registry);

    // Create arbiter pool for spawning actors across threads
    let mut arbiter_pool = ArbiterPool::new().await?;

    // The specialized-node invite topic is subscribed by every node but
    // is not an overlay we want per-key rendezvous registration for —
    // reserve it so the discovery layer doesn't map it to a rendezvous
    // key (which would register all nodes under one key and recreate the
    // global fan-out).
    let reserved_topics = BTreeSet::from([config.specialized_node.invite_topic.clone()]);

    // Create NetworkManager with channel-based dispatcher for reliable
    // event delivery. The datastore handle backs the peer-address cache
    // (datastore-backed peerstore) so a restart can dial known
    // co-members immediately for fast reconnect.
    let network_manager = NetworkManager::new(
        &config.network,
        Arc::new(network_event_sender),
        &mut registry,
        reserved_topics,
        Some(datastore.clone()),
    )
    .await?;

    let network_client = NetworkClient::new(network_recipient.clone());

    let _ignored = Actor::start_in_arbiter(&arbiter_pool.get().await?, move |ctx| {
        assert!(network_recipient.init(ctx), "failed to initialize");
        network_manager
    });

    info!(
        topic = %config.specialized_node.invite_topic,
        "Subscribing to specialized node invite topic"
    );
    let _ignored = network_client
        .subscribe(IdentTopic::new(
            config.specialized_node.invite_topic.clone(),
        ))
        .await?;

    // Increased buffer sizes for better burst handling and concurrency
    // 256 events: supports more concurrent WebSocket clients
    // 64 sync requests: handles burst context joins/syncs
    let (event_sender, _) = broadcast::channel(256);

    let (ctx_sync_tx, ctx_sync_rx) = mpsc::channel(64);
    let (ns_sync_tx, ns_sync_rx) = mpsc::channel(16);
    let (ns_join_tx, ns_join_rx) = mpsc::channel(16);
    let (open_subgroup_join_tx, open_subgroup_join_rx) = mpsc::channel(16);

    let sync_client = SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

    // Channel for the execute path to notify the node about locally-
    // applied deltas so the in-memory DeltaStore stays current without
    // re-scanning the DB on every interval sync. Drained by a task
    // spawned once `node_state` is available (below).
    let (local_delta_tx, mut local_delta_rx) = mpsc::channel(256);

    let node_client = NodeClient::new(
        datastore.clone(),
        blob_manager.clone(),
        network_client.clone(),
        node_recipient.clone(),
        event_sender,
        sync_client,
        config.specialized_node.invite_topic.clone(),
        Some(local_delta_tx),
    );

    let context_client = ContextClient::new(
        datastore.clone(),
        node_client.clone(),
        context_recipient.clone(),
    );

    // Shared unified-op projection registry (cutover-flip prerequisite): one
    // instance fed by the context manager and read by the node at the
    // data-write decision. Threaded into both below.
    let scope_projections = std::sync::Arc::new(std::sync::Mutex::new(
        calimero_context::scope_projection::ScopeProjections::new(),
    ));

    let context_manager = ContextManager::new(
        datastore.clone(),
        node_client.clone(),
        context_client.clone(),
        Some(&mut registry),
    )
    .with_vm_limits(config.vm_limits)
    .with_migration_v2(config.context.migration_v2)
    .with_scope_projections(std::sync::Arc::clone(&scope_projections));

    let _ignored = Actor::start_in_arbiter(&arbiter_pool.get().await?, move |ctx| {
        assert!(context_recipient.init(ctx), "failed to initialize");
        context_manager
    });

    let mut node_state =
        crate::NodeState::new(config.specialized_node.accept_mock_tee, config.mode);
    // Share the one registry the context manager feeds, so the node side reads
    // the same projection at the data-write decision.
    node_state.scope_projections = std::sync::Arc::clone(&scope_projections);

    // Periodic gauge-snapshot tick — once per `METRICS_TICK_INTERVAL`,
    // reads NodeState DashMap sizes and process resource counters and
    // updates the registered gauges. Gives operators a dashboard view
    // of buffer / cache / session counts without instrumenting every
    // mutation site.
    let _metrics_tick =
        crate::node_metrics::spawn_metrics_tick(node_metrics.clone(), node_state.clone());

    // Hydrate the persistent peer-identity cache from disk and seed the
    // in-memory reverse view, so anchor-preferred sync selection has a
    // membership signal on a cold cache instead of waiting for live
    // traffic to refill it. Then snapshot it back periodically.
    crate::peer_identity_persist::hydrate(&node_state, &datastore);
    // Apply gossipsub scores for the just-hydrated members immediately,
    // rather than waiting for the first snapshot tick (~30s).
    crate::peer_identity_persist::reconcile_peer_scores(&node_state, &network_client, true);
    let _peer_identity_tick = crate::peer_identity_persist::spawn_snapshot_tick(
        node_state.clone(),
        datastore.clone(),
        network_client.clone(),
    );
    // Drop removed members from the cache promptly on `MemberRemoved`,
    // rather than waiting for their entries to age out via TTL.
    let _peer_identity_invalidation =
        crate::peer_identity_persist::spawn_invalidation_task(node_state.clone());

    // Drain locally-applied delta notifications from the execute path
    // and register them into the in-memory DeltaStore. Replaces the
    // per-interval-sync `load_persisted_deltas` rescan that existed
    // solely to catch up on execute-side writes.
    {
        let delta_stores = node_state.delta_stores_handle();
        let _drainer = tokio::spawn(async move {
            while let Some(msg) = local_delta_rx.recv().await {
                // Clone the DeltaStore value out of the DashMap and
                // drop the Ref before awaiting — holding a shard lock
                // across `.await` would block other context shards.
                let store = match delta_stores.get(&msg.context_id) {
                    Some(entry) => entry.value().clone(),
                    None => {
                        // Benign race: execute finished before the
                        // DeltaStore entry was created (e.g. isolated
                        // single-node test). Startup
                        // `load_persisted_deltas` will pick the row up
                        // when the store is eventually constructed.
                        tracing::debug!(
                            context_id = %msg.context_id,
                            "no DeltaStore for local applied delta, skipping"
                        );
                        continue;
                    }
                };
                let delta = calimero_dag::CausalDelta {
                    id: msg.delta_id,
                    parents: msg.parents,
                    payload: msg.actions,
                    hlc: msg.hlc,
                    expected_root_hash: msg.expected_root_hash,
                    kind: calimero_dag::DeltaKind::Regular,
                };
                match store.add_local_applied_delta(delta).await {
                    Ok(cascaded_events) if !cascaded_events.is_empty() => {
                        // Cascaded children's DB state + dag_heads were
                        // persisted inside add_local_applied_delta; the
                        // events list returned here carries payloads that
                        // still need handler execution. Today the drainer
                        // has no line into NodeClients / NodeManager, so
                        // we rely on the restart-replay contract (#2185):
                        // records stay `applied: true, events: Some(..)`
                        // until the next `load_persisted_deltas` surfaces
                        // them. Log at info so missed handler runs are
                        // observable while plumbing is added.
                        tracing::info!(
                            context_id = %msg.context_id,
                            cascaded_count = cascaded_events.len(),
                            "Cascaded events persisted; awaiting restart replay for handler execution"
                        );
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(
                            error = ?e,
                            context_id = %msg.context_id,
                            "failed to register local applied delta in DAG"
                        );
                    }
                }
            }
        });
    }

    let mut sync_manager = SyncManager::new(
        config.sync,
        node_client.clone(),
        context_client.clone(),
        network_client.clone(),
        node_state.clone(),
        ctx_sync_rx,
        ns_sync_rx,
        ns_join_rx,
        open_subgroup_join_rx,
    );

    // Attach the sync-protocol metrics collector. Must happen before any
    // clones are taken — every responder/initiator clone shares the
    // recording handle via `Arc`.
    sync_manager.set_metrics(sync_metrics);

    // Spin up the dedicated StateDelta actor on its own Arbiter
    // BEFORE constructing NodeManager so the sender can be threaded
    // through. The arbiter is drawn from `arbiter_pool` (which owns
    // the Actix `System`); see issue #2299.
    let state_delta_arbiter = arbiter_pool.get().await?;
    let state_delta_tx =
        start_state_delta_actor(&state_delta_arbiter, STATE_DELTA_CHANNEL_CAPACITY);

    // Spin up the dedicated SyncSession actor on its own Arbiter
    // (#2316). The actor receives a `SyncManager` clone so it can
    // call `handle_opened_stream` (responder) and
    // `perform_interval_sync` (initiator) without contending with
    // the `NodeManager` arbiter or the `SyncManager::start` select
    // loop. Initiator results are forwarded back to the original
    // `sync_manager` via `session_result_tx`/`session_result_rx`
    // so per-context tracking state still updates.
    let sync_session_arbiter = arbiter_pool.get().await?;
    // Unbounded result channel: a dropped result would leave the
    // per-context `last_sync = None` forever and stall that context
    // (same failure shape as the C1 dispatch stall). Result messages
    // are small (~32 bytes); bounding adds risk without payoff.
    let (session_result_tx, session_result_rx) = tokio::sync::mpsc::unbounded_channel();
    let sync_session_tx = start_sync_session_actor(
        &sync_session_arbiter,
        SYNC_SESSION_CHANNEL_CAPACITY,
        config.sync.max_concurrent,
        sync_manager.clone(),
        config.sync.session_deadline,
        Some(session_result_tx),
        &mut registry,
    );
    sync_manager.set_session_handles(sync_session_tx.clone(), session_result_rx);

    // #2319: divergence counter — the hash-heartbeat handler bumps this
    // whenever it sees a peer with the same DAG heads but a different
    // storage root hash. Exposed as `sync_root_hash_divergence_detected_total`.
    let divergence_detected = prometheus_client::metrics::counter::Counter::default();
    registry.sub_registry_with_prefix("sync").register(
        "root_hash_divergence_detected_total",
        "Times the hash-heartbeat observed a peer with the same DAG heads but a different storage root hash (#2319)",
        divergence_detected.clone(),
    );

    let node_manager = NodeManager::new(
        blob_store.clone(),
        sync_manager.clone(),
        context_client.clone(),
        node_client.clone(),
        datastore.clone(),
        node_state.clone(),
        state_delta_tx,
        sync_session_tx,
        divergence_detected,
    );

    // Start NodeManager actor and get its address
    let node_manager_addr = Actor::start_in_arbiter(&arbiter_pool.get().await?, move |ctx| {
        assert!(node_recipient.init(ctx), "failed to initialize");
        node_manager
    });

    // Start the network event bridge in a dedicated tokio task
    // This bridges the channel to NodeManager, ensuring reliable message delivery
    // by avoiding cross-arbiter message passing issues
    let bridge = NetworkEventBridge::new(network_event_receiver, node_manager_addr);
    let bridge_shutdown = bridge.shutdown_handle();
    let bridge_handle = tokio::spawn(bridge.run());

    let server = calimero_server::start(
        config.server.clone(),
        context_client.clone(),
        node_client.clone(),
        datastore.clone(),
        registry,
    );

    // Start garbage collection actor
    let gc_interval = Duration::from_secs(
        config.gc_interval_secs.unwrap_or(12 * 3600), // Default: 12 hours
    );
    let gc = GarbageCollector::new(datastore.clone(), gc_interval);

    let _ignored = Actor::start_in_arbiter(&arbiter_pool.get().await?, move |_ctx| gc);

    // Start DAG compaction actor (issue #2026). Enabled by default; bounds
    // on-disk delta growth unless `[dag_compaction] enabled = false`.
    if config.dag_compaction.enabled {
        // Fail fast on a misconfigured-but-enabled config rather than silently
        // skipping compaction. With compaction default-on, a silent skip (e.g.
        // an operator setting `retain_recent_count >= min_deltas_before_compact`
        // or a zero `check_interval`) would let delta-log growth go unbounded
        // unnoticed — the opposite of this feature's intent. The default config
        // is always valid, so this only trips a deliberate misconfiguration.
        eyre::ensure!(
            config.dag_compaction.is_valid(),
            "invalid [dag_compaction] config: retain_recent_count ({}) must be \
             < min_deltas_before_compact ({}) and check_interval must be non-zero",
            config.dag_compaction.retain_recent_count,
            config.dag_compaction.min_deltas_before_compact,
        );
        let compactor = DagCompactor::new(node_state.delta_stores_handle(), config.dag_compaction);
        let _ignored = Actor::start_in_arbiter(&arbiter_pool.get().await?, move |_ctx| compactor);
    }

    let mut sync = pin!(sync_manager.start());
    let mut server = tokio::spawn(server);
    let mut bridge = bridge_handle;

    info!("Node started successfully");

    loop {
        tokio::select! {
            _ = &mut sync => {},
            res = &mut server => res??,
            res = &mut bridge => {
                // Bridge task completed (channel closed or shutdown signal)
                tracing::warn!("Network event bridge stopped: {:?}", res);
            }
            res = &mut arbiter_pool.system_handle => {
                // Signal bridge shutdown before exiting. The
                // StateDelta arbiter handle (`state_delta_arbiter`)
                // lives until this function returns; the underlying
                // Actix arbiter thread is owned by the System and
                // shuts down with it.
                bridge_shutdown.notify_one();
                break res?;
            }
        }
    }
}
