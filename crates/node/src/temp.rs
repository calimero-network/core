use std::pin::pin;

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
use calimero_store::db::RocksDB;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use camino::Utf8PathBuf;
use eyre::{eyre, OptionExt};
use futures_util::{stream, TryStreamExt};
use libp2p::identity::Keypair;
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info};

use crate::interactive_cli::handle_line;
// use crate::sync::SyncConfig;
use crate::NodeManager;

pub struct NodeConfig {
    pub home: Utf8PathBuf,
    pub identity: Keypair,
    pub network: NetworkConfig,
    // pub sync: SyncConfig,
    pub datastore: StoreConfig,
    pub blobstore: BlobStoreConfig,
    pub context: ContextConfig,
    pub server: ServerConfig,
}

pub async fn start(config: NodeConfig) -> eyre::Result<()> {
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

        let arb = Arbiter::current();

        let spawned = arb.clone().spawn({
            let tx = tx.clone();

            let task = async move {
                tx.send(Ok(Some(arb))).await?;

                loop {
                    tx.send(Ok(None)).await?;
                    tx.send(Ok(Some(Arbiter::new().handle()))).await?;
                }
            };

            async {
                let _ignored: eyre::Result<()> = task.await;
            }
        });

        if !spawned {
            let _ignored = tx.blocking_send(Err(eyre!("failed to derive arbiters")));
        }

        if let Err(err) = system.run() {
            let _ignored = tx.blocking_send(Err(err.into()));
        }
    });

    let arbs = stream::poll_fn(|cx| rx.poll_recv(cx)).try_filter_map(async |t| Ok(t));

    let mut arbs = pin!(arbs);

    let network_manager = NetworkManager::new(&config.network, network_event_recipient)?;

    let _ignored = Actor::start_in_arbiter(
        &arbs.try_next().await?.ok_or_eyre("failed to get arbiter")?,
        {
            let network_recipient = network_recipient.clone();

            move |ctx| {
                assert!(network_recipient.init(ctx));
                network_manager
            }
        },
    );

    let network_client = NetworkClient::new(network_recipient);

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

    let _ignored = Actor::start_in_arbiter(
        &arbs.try_next().await?.ok_or_eyre("failed to get arbiter")?,
        {
            let context_recipient = context_recipient.clone();

            move |ctx| {
                assert!(context_recipient.init(ctx));
                context_manager
            }
        },
    );

    let node_manager = NodeManager::new(
        // config.sync,
        datastore.clone(),
        blobstore.clone(),
        context_client.clone(),
        node_client.clone(),
    );

    let _ignored = Actor::start_in_arbiter(
        &arbs.try_next().await?.ok_or_eyre("failed to get arbiter")?,
        {
            let node_recipient = node_recipient.clone();

            move |ctx| {
                assert!(node_recipient.init(ctx));
                node_manager
            }
        },
    );

    let mut server = tokio::spawn(calimero_server::start(
        config.server,
        context_client.clone(),
        node_client.clone(),
        datastore.clone(),
    ));

    let mut stdin = BufReader::new(io::stdin()).lines();

    loop {
        tokio::select! {
            res = &mut system => res?,
            res = &mut server => res??,
            line = stdin.next_line() => {
                let Some(line) = line? else {
                    continue;
                };

                let it = handle_line(
                    context_client.clone(),
                    node_client.clone(),
                    datastore.clone(),
                    line,
                );

                let _ignored = tokio::spawn(async {
                    if let Err(err) = it.await {
                        error!(%err, "failed handling user command");
                    }
                });
            }
        }
    }
}
