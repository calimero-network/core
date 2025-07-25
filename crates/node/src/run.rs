use std::collections::HashMap;
use std::io::{self, BufRead, BufReader};
use std::pin::{pin, Pin};
use std::sync::Arc;
use std::thread;

use actix::{Actor, Arbiter, System};
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
use eyre::{OptionExt, WrapErr};
use futures_util::{stream, StreamExt};
use libp2p::identity::Keypair;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, event_enabled, info, Level};

use crate::interactive_cli::handle_line;
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
    pub protocol_config: HashMap<String, String>,
}

pub async fn start(config: NodeConfig) -> eyre::Result<()> {
    for (key, value) in &config.protocol_config {
        std::env::set_var(key, value);
    }

    let peer_id = config.identity.public().to_peer_id();

    info!("Peer ID: {}", peer_id);

    let datastore = Store::open::<RocksDB>(&config.datastore)?;

    let blobstore = BlobManager::new(datastore.clone(), FileSystem::new(&config.blobstore).await?);

    let node_recipient = LazyRecipient::new();
    let network_recipient = LazyRecipient::new();
    let context_recipient = LazyRecipient::new();
    let network_event_recipient = LazyRecipient::new();

    let (tx, mut rx) = mpsc::channel(1);

    let mut system = tokio::task::spawn_blocking(move || {
        let system = System::new();

        let _ignored = system.runtime().spawn({
            let task = async move {
                let mut arb = Arbiter::current();

                loop {
                    tx.send(Some(arb)).await?;

                    tx.send(None).await?;
                    tx.send(None).await?;

                    arb = Arbiter::new().handle();
                }
            };

            async {
                let _ignored: eyre::Result<()> = task.await;

                System::current().stop();
            }
        });

        system
            .run()
            .wrap_err("the actix subsystem ran into an error")
    });

    let mut new_arbiter = {
        let mut arbs = stream::poll_fn(|cx| rx.poll_recv(cx)).filter_map(async |t| t);

        async move || {
            let mut arbs = unsafe { Pin::new_unchecked(&mut arbs) };

            arbs.next().await.ok_or_eyre("failed to get arbiter")
        }
    };

    let network_manager =
        NetworkManager::new(&config.network, network_event_recipient.clone()).await?;

    let network_client = NetworkClient::new(network_recipient.clone());

    let _ignored = Actor::start_in_arbiter(&new_arbiter().await?, move |ctx| {
        assert!(network_recipient.init(ctx), "failed to initialize");
        network_manager
    });

    let (event_sender, _) = broadcast::channel(32);

    let node_client = NodeClient::new(
        datastore.clone(),
        blobstore.clone(),
        network_client.clone(),
        node_recipient.clone(),
        event_sender,
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
    );

    let _ignored = Actor::start_in_arbiter(&new_arbiter().await?, move |ctx| {
        assert!(context_recipient.init(ctx), "failed to initialize");
        context_manager
    });

    let sync_manager = SyncManager::new(
        config.sync,
        node_client.clone(),
        context_client.clone(),
        network_client.clone(),
    );

    let node_manager = NodeManager::new(
        blobstore.clone(),
        sync_manager.clone(),
        context_client.clone(),
        node_client.clone(),
    );

    let _ignored = Actor::start_in_arbiter(&new_arbiter().await?, move |ctx| {
        assert!(node_recipient.init(ctx), "failed to initialize");
        assert!(network_event_recipient.init(ctx), "failed to initialize");
        node_manager
    });

    let server = calimero_server::start(
        config.server.clone(),
        context_client.clone(),
        node_client.clone(),
        datastore.clone(),
    );

    let config = Arc::new(config);

    let mut sync = pin!(sync_manager.start());
    let mut server = tokio::spawn(server);

    let (lines_tx, mut lines) = mpsc::channel(1);

    let _ignored = thread::spawn(move || {
        let stdin = BufReader::new(io::stdin());

        for line in stdin.lines() {
            let line = line.expect("unable to receive line from stdin");

            lines_tx.blocking_send(line).expect("unable to send line");
        }
    });

    loop {
        tokio::select! {
            _ = &mut sync => {},
            res = &mut server => res??,
            res = &mut system => break res?,
            line = lines.recv() => {
                let Some(line) = line else {
                    continue;
                };

                let it = handle_line(
                    context_client.clone(),
                    node_client.clone(),
                    datastore.clone(),
                    config.clone(),
                    line,
                );

                let _ignored = tokio::spawn(async {
                    if let Err(err) = it.await {
                        if event_enabled!(Level::DEBUG) {
                            error!(?err, "failed handling user command");
                        } else {
                            error!(%err, "failed handling user command");
                        }
                    }
                });
            }
        }
    }
}
