//! Node startup and initialization.
//!
//! **Purpose**: Bootstraps the node with all required services and actors.
//! **Main Function**: `start(NodeConfig)` - initializes and runs the node.

use std::pin::pin;
use std::sync::Arc;
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
use crate::gc::GarbageCollector;
use crate::network_event_channel::{self, NetworkEventChannelConfig};
use crate::network_event_processor::NetworkEventBridge;
use crate::sync::{SyncConfig, SyncManager};
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
    pub mode: NodeMode,
    pub specialized_node: SpecializedNodeConfig,
}

pub async fn start(config: NodeConfig) -> eyre::Result<()> {
    let mut registry = <Registry>::default();

    let peer_id = config.identity.public().to_peer_id();

    info!("Peer ID: {}", peer_id);

    // Open datastore with optional encryption
    let datastore = if let Some(ref key) = config.datastore.encryption_key {
        info!("Opening encrypted datastore");
        let inner_db = RocksDB::open(&config.datastore)?;
        let encrypted_db = EncryptedDatabase::wrap(inner_db, key.clone())?;
        Store::new(std::sync::Arc::new(encrypted_db))
    } else {
        Store::open::<RocksDB>(&config.datastore)?
    };

    let blobstore = BlobManager::new(datastore.clone(), FileSystem::new(&config.blobstore).await?);

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

    // Create NetworkManager with channel-based dispatcher for reliable event delivery
    let network_manager = NetworkManager::new(
        &config.network,
        Arc::new(network_event_sender),
        &mut registry,
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

    let node_client = NodeClient::new(
        datastore.clone(),
        blobstore.clone(),
        network_client.clone(),
        node_recipient.clone(),
        event_sender,
        ctx_sync_tx,
        config.specialized_node.invite_topic.clone(),
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

    let node_state = crate::NodeState::new(config.specialized_node.accept_mock_tee, config.mode);

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

    let mut sync = pin!(sync_manager.start());
    let mut server = tokio::spawn(server);
    let mut bridge = bridge_handle;

    info!("Node started successfully");

    loop {
        tokio::select! {
            _ = &mut sync => {},
            res = &mut server => res??,
            res = &mut bridge => {
                match res {
                    Ok(()) => info!("Network event bridge stopped gracefully"),
                    Err(e) => tracing::error!(?e, "Network event bridge panicked"),
                }
            },
            res = &mut arbiter_pool.system_handle => {
                // Signal bridge shutdown before exiting
                bridge_shutdown.notify_one();
                break res?;
            },
        }
    }
}
