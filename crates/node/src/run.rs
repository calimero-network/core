//! Node startup and initialization.
//!
//! **Purpose**: Bootstraps the node with all required services and actors.
//! **Main Function**: `start(NodeConfig)` - initializes and runs the node.

use std::pin::pin;
use std::time::Duration;

use actix::Actor;
use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager, FileSystem};
use calimero_context::config::ContextConfig;
use calimero_context::ContextManager;
use calimero_context_config::client::Client as ExternalClient;
use calimero_context_primitives::client::ContextClient;
use calimero_network::NetworkManager;
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::config::NetworkConfig;
use calimero_node_primitives::client::NodeClient;
use calimero_server::config::ServerConfig;
use calimero_store::config::StoreConfig;
use calimero_store::Store;
use calimero_store_rocksdb::RocksDB;
use calimero_utils_actix::LazyRecipient;
use camino::Utf8PathBuf;
use libp2p::gossipsub::IdentTopic;
use libp2p::identity::Keypair;
use prometheus_client::registry::Registry;
use tokio::sync::{broadcast, mpsc};
use tracing::info;

use crate::arbiter_pool::ArbiterPool;
use crate::gc::GarbageCollector;
use crate::sync::{SyncConfig, SyncManager};
use crate::NodeManager;

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
}

pub async fn start(config: NodeConfig) -> eyre::Result<()> {
    let mut registry = <Registry>::default();

    let peer_id = config.identity.public().to_peer_id();

    info!("Peer ID: {}", peer_id);

    let datastore = Store::open::<RocksDB>(&config.datastore)?;

    let blobstore = BlobManager::new(datastore.clone(), FileSystem::new(&config.blobstore).await?);

    let node_recipient = LazyRecipient::new();
    let network_recipient = LazyRecipient::new();
    let context_recipient = LazyRecipient::new();
    let network_event_recipient = LazyRecipient::new();

    // Create arbiter pool for spawning actors across threads
    let mut arbiter_pool = ArbiterPool::new().await?;

    let network_manager = NetworkManager::new(
        &config.network,
        network_event_recipient.clone(),
        &mut registry,
    )
    .await?;

    let network_client = NetworkClient::new(network_recipient.clone());

    let _ignored = Actor::start_in_arbiter(&arbiter_pool.get().await?, move |ctx| {
        assert!(network_recipient.init(ctx), "failed to initialize");
        network_manager
    });

    let _ignored = network_client
        .subscribe(IdentTopic::new("meta_topic".to_owned()))
        .await?;

    // Increased buffer sizes for better burst handling and concurrency
    // 256 events: supports more concurrent WebSocket clients
    // 64 sync requests: handles burst context joins/syncs
    let (event_sender, _) = broadcast::channel(256);

    let (ctx_sync_tx, ctx_sync_rx) = mpsc::channel(64);

    let node_client = NodeClient::new(
        datastore.clone(),
        blobstore.clone(),
        network_client.clone(),
        node_recipient.clone(),
        event_sender,
        ctx_sync_tx,
    );

    let external_client = ExternalClient::from_config(&config.context.client);

    let context_client = ContextClient::new(
        datastore.clone(),
        node_client.clone(),
        external_client,
        context_recipient.clone(),
    );

    let context_manager = ContextManager::new(
        datastore.clone(),
        node_client.clone(),
        context_client.clone(),
        config.context.client.clone(),
        Some(&mut registry),
    );

    let _ignored = Actor::start_in_arbiter(&arbiter_pool.get().await?, move |ctx| {
        assert!(context_recipient.init(ctx), "failed to initialize");
        context_manager
    });

    let node_state = crate::NodeState::new();

    let sync_manager = SyncManager::new(
        config.sync,
        node_client.clone(),
        context_client.clone(),
        network_client.clone(),
        node_state.clone(),
        ctx_sync_rx,
    );

    let node_manager = NodeManager::new(
        blobstore.clone(),
        sync_manager.clone(),
        context_client.clone(),
        node_client.clone(),
        node_state.clone(),
    );

    let _ignored = Actor::start_in_arbiter(&arbiter_pool.get().await?, move |ctx| {
        assert!(node_recipient.init(ctx), "failed to initialize");
        assert!(network_event_recipient.init(ctx), "failed to initialize");
        node_manager
    });

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

    let mut sync = pin!(sync_manager.start());
    let mut server = tokio::spawn(server);

    info!("Node started successfully");

    loop {
        tokio::select! {
            _ = &mut sync => {},
            res = &mut server => res??,
            res = &mut arbiter_pool.system_handle => break res?,
        }
    }
}
