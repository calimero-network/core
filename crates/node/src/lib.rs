#![allow(clippy::print_stdout)]

use core::future::{pending, Future};
use core::pin::Pin;

use calimero_blobstore::{BlobManager, FileSystem};
use calimero_context::config::ApplicationConfig;
use calimero_context::ContextManager;
use calimero_network::client::NetworkClient;
use calimero_network::config::NetworkConfig;
use calimero_network::types::{NetworkEvent, PeerId};
use calimero_node_primitives::{CallError, MutateCallError, NodeType, QueryCallError};
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::events::{
    ApplicationEvent, ApplicationEventPayload, ExecutedTransactionPayload, NodeEvent, OutcomeEvent,
    OutcomeEventPayload, PeerJoinedPayload,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::transaction::Transaction;
use calimero_runtime::logic::{Outcome, VMContext, VMLimits};
use calimero_runtime::Constraint;
use calimero_server::admin::utils::context::{create_context, join_context};
use calimero_server::config::ServerConfig;
use calimero_store::config::StoreConfig;
use calimero_store::db::RocksDB;
use calimero_store::key::{
    ApplicationMeta as ApplicationMetaKey, ContextMeta as ContextMetaKey,
    ContextState as ContextStateKey, ContextTransaction as ContextTransactionKey,
};
use calimero_store::types::{ContextMeta, ContextTransaction};
use calimero_store::Store;
use camino::Utf8PathBuf;
use eyre::{bail, eyre, Result as EyreResult};
use libp2p::gossipsub::{IdentTopic, Message, TopicHash};
use libp2p::identity::Keypair;
use owo_colors::OwoColorize;
use serde_json::{
    from_slice as from_json_slice, from_str as from_json_str, to_vec as to_json_vec, Value,
};
use tokio::io::{stdin, AsyncBufReadExt, BufReader};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{interval_at, Instant};
use tokio::{select, spawn};
use tracing::{debug, error, info, warn};

use crate::runtime_compat::RuntimeCompatStore;
use crate::transaction_pool::{TransactionPool, TransactionPoolEntry};
use crate::types::{PeerAction, TransactionConfirmation, TransactionRejection};

pub mod catchup;
pub mod runtime_compat;
pub mod transaction_pool;
pub mod types;

type BoxedFuture<T> = Pin<Box<dyn Future<Output = T>>>;

#[derive(Debug)]
#[non_exhaustive]
pub struct NodeConfig {
    pub home: Utf8PathBuf,
    pub identity: Keypair,
    pub node_type: NodeType,
    pub application: ApplicationConfig,
    pub network: NetworkConfig,
    pub server: ServerConfig,
    pub store: StoreConfig,
}

impl NodeConfig {
    #[must_use]
    pub const fn new(
        home: Utf8PathBuf,
        node_type: NodeType,
        identity: Keypair,
        store: StoreConfig,
        application: ApplicationConfig,
        network: NetworkConfig,
        server: ServerConfig,
    ) -> Self {
        Self {
            home,
            identity,
            node_type,
            application,
            network,
            server,
            store,
        }
    }
}

#[derive(Debug)]
pub struct Node {
    id: PeerId,
    typ: NodeType,
    store: Store,
    tx_pool: TransactionPool,
    ctx_manager: ContextManager,
    network_client: NetworkClient,
    node_events: broadcast::Sender<NodeEvent>,
    // --
    nonce: u64,
}

pub async fn start(config: NodeConfig) -> EyreResult<()> {
    let peer_id = config.identity.public().to_peer_id();

    info!("Peer ID: {}", peer_id);

    let (node_events, _) = broadcast::channel(32);

    let (network_client, mut network_events) = calimero_network::run(&config.network).await?;

    let store = Store::open::<RocksDB>(&config.store)?;

    let blob_manager = BlobManager::new(
        store.clone(),
        FileSystem::new(&config.application.dir).await?,
    );

    let ctx_manager =
        ContextManager::start(store.clone(), blob_manager, network_client.clone()).await?;

    let mut node = Node::new(
        &config,
        network_client.clone(),
        node_events.clone(),
        ctx_manager.clone(),
        store.clone(),
    );

    let (server_sender, mut server_receiver) = mpsc::channel(32);

    #[allow(trivial_casts)]
    let mut server = Box::pin(calimero_server::start(
        config.server,
        server_sender,
        ctx_manager,
        node_events,
        store,
    )) as BoxedFuture<EyreResult<()>>;

    let mut stdin = BufReader::new(stdin()).lines();

    match network_client
        .subscribe(IdentTopic::new("meta_topic"))
        .await
    {
        Ok(_) => info!("Subscribed to meta topic"),
        Err(err) => {
            error!("{}: {:?}", "Error subscribing to meta topic", err);
            bail!("Failed to subscribe to meta topic: {:?}", err)
        }
    };

    let mut catchup_interval_tick = interval_at(
        Instant::now()
            .checked_add(config.network.catchup.initial_delay)
            .ok_or_else(|| eyre!("Overflow when calculating initial catchup interval delay"))?,
        config.network.catchup.interval,
    );

    #[allow(clippy::redundant_pub_crate)]
    loop {
        select! {
            event = network_events.recv() => {
                let Some(event) = event else {
                    break;
                };
                node.handle_event(event).await?;
            }
            line = stdin.next_line() => {
                if let Some(line) = line? {
                    handle_line(&mut node, line).await?;
                }
            }
            result = &mut server => {
                result?;
                server = Box::pin(pending());
                continue;
            }
            Some((context_id, method, payload, write, executor_public_key, outcome_sender)) = server_receiver.recv() => {
                node.handle_call(context_id, method, payload, write, executor_public_key, outcome_sender).await;
            }
            _ = catchup_interval_tick.tick() => node.handle_interval_catchup().await,
        }
    }

    Ok(())
}

// TODO: Consider splitting this long function into multiple parts.
#[allow(clippy::too_many_lines)]
async fn handle_line(node: &mut Node, line: String) -> EyreResult<()> {
    let (command, args) = match line.split_once(' ') {
        Some((method, payload)) => (method, Some(payload)),
        None => (line.as_str(), None),
    };

    #[allow(non_snake_case)]
    let IND = " │".yellow();

    // TODO: should be replaced with RPC endpoints
    match command {
        "call" => {
            if let Some((context_id, rest)) = args.and_then(|args| args.split_once(' ')) {
                let (method, rest) = rest.split_once(' ').unwrap_or((rest, "{}"));
                let (payload, executor_key) = rest.split_once(' ').unwrap_or((rest, ""));

                match from_json_str::<Value>(payload) {
                    Ok(_) => {
                        let (outcome_sender, outcome_receiver) = oneshot::channel();

                        let context_id = context_id.parse()?;

                        let Ok(Some(context)) = node.ctx_manager.get_context(&context_id) else {
                            println!("{IND} Context not found: {context_id}");
                            return Ok(());
                        };

                        // Parse the executor's public key if provided
                        let executor_public_key = if executor_key.is_empty() {
                            return Err(eyre!("Executor public key is required"));
                        } else {
                            bs58::decode(executor_key)
                                .into_vec()
                                .map_err(|_| eyre!("Invalid executor public key"))?
                                .try_into()
                                .map_err(|_| eyre!("Executor public key must be 32 bytes"))?
                        };

                        let tx_hash = match node
                            .call_mutate(
                                context,
                                method.to_owned(),
                                payload.as_bytes().to_owned(),
                                executor_public_key,
                                outcome_sender,
                            )
                            .await
                        {
                            Ok(tx_hash) => tx_hash,
                            Err(e) => {
                                println!("{IND} Failed to execute transaction: {e}");
                                return Ok(());
                            }
                        };

                        println!("{IND} Scheduled Transaction! {tx_hash:?}");

                        drop(spawn(async move {
                            if let Ok(outcome_result) = outcome_receiver.await {
                                println!("{IND} {tx_hash:?}");

                                match outcome_result {
                                    Ok(outcome) => {
                                        match outcome.returns {
                                            Ok(result) => match result {
                                                Some(result) => {
                                                    println!("{IND}   Return Value:");
                                                    #[allow(clippy::option_if_let_else)]
                                                    let result = if let Ok(value) =
                                                        from_json_slice::<Value>(&result)
                                                    {
                                                        format!(
                                                            "(json): {}",
                                                            format!("{value:#}")
                                                                .lines()
                                                                .map(|line| line.cyan().to_string())
                                                                .collect::<Vec<_>>()
                                                                .join("\n")
                                                        )
                                                    } else {
                                                        format!("(raw): {:?}", result.cyan())
                                                    };

                                                    for line in result.lines() {
                                                        println!("{IND}     > {line}");
                                                    }
                                                }
                                                None => println!("{IND}   (No return value)"),
                                            },
                                            Err(err) => {
                                                let err = format!("{err:#?}");

                                                println!("{IND}   Error:");
                                                for line in err.lines() {
                                                    println!("{IND}     > {}", line.yellow());
                                                }
                                            }
                                        }

                                        if !outcome.logs.is_empty() {
                                            println!("{IND}   Logs:");

                                            for log in outcome.logs {
                                                println!("{IND}     > {}", log.cyan());
                                            }
                                        }
                                    }
                                    Err(err) => {
                                        let err = format!("{err:#?}");

                                        println!("{IND}   Error:");
                                        for line in err.lines() {
                                            println!("{IND}     > {}", line.yellow());
                                        }
                                    }
                                }
                            }
                        }));
                    }
                    Err(e) => {
                        println!("{IND} Failed to parse payload: {e}");
                    }
                }
            } else {
                println!(
                    "{IND} Usage: call <Context ID> <Method> <JSON Payload> <Executor Public Key<"
                );
            }
        }
        "gc" => {
            if node.tx_pool.transactions.is_empty() {
                println!("{IND} Transaction pool is empty.");
            } else {
                println!(
                    "{IND} Garbage collecting {} transactions.",
                    node.tx_pool.transactions.len().cyan()
                );
                node.tx_pool = TransactionPool::default();
            }
        }
        "pool" => {
            if node.tx_pool.transactions.is_empty() {
                println!("{IND} Transaction pool is empty.");
            }
            for (hash, entry) in &node.tx_pool.transactions {
                println!("{IND} • {:?}", hash.cyan());
                println!("{IND}     Sender: {}", entry.sender.cyan());
                println!("{IND}     Method: {:?}", entry.transaction.method.cyan());
                println!("{IND}     Payload:");
                #[allow(clippy::option_if_let_else)]
                let payload =
                    if let Ok(value) = from_json_slice::<Value>(&entry.transaction.payload) {
                        format!(
                            "(json): {}",
                            format!("{value:#}")
                                .lines()
                                .map(|line| line.cyan().to_string())
                                .collect::<Vec<_>>()
                                .join("\n")
                        )
                    } else {
                        format!("(raw): {:?}", entry.transaction.payload.cyan())
                    };

                for line in payload.lines() {
                    println!("{IND}       > {line}");
                }
                println!("{IND}     Prior: {:?}", entry.transaction.prior_hash.cyan());
            }
        }
        "peers" => {
            println!(
                "{IND} Peers (General): {:#?}",
                node.network_client.peer_count().await.cyan()
            );

            if let Some(args) = args {
                // TODO: implement print all and/or specific topic
                let topic = TopicHash::from_raw(args);
                println!(
                    "{IND} Peers (Session) for Topic {}: {:#?}",
                    topic.clone(),
                    node.network_client.mesh_peer_count(topic).await.cyan()
                );
            }
        }
        "store" => {
            // todo! revisit: get specific context state
            // todo! test this

            println!(
                "{IND} {c1:44} | {c2:44} | Value",
                c1 = "Context ID",
                c2 = "State Key",
            );

            let handle = node.store.handle();

            for (k, v) in handle.iter::<ContextStateKey>()?.entries() {
                let (k, v) = (k?, v?);
                let (cx, state_key) = (k.context_id(), k.state_key());
                let sk = Hash::from(state_key);
                let entry = format!("{c1:44} | {c2:44}| {c3:?}", c1 = cx, c2 = sk, c3 = v.value);
                for line in entry.lines() {
                    println!("{IND} {}", line.cyan());
                }
            }
        }
        "application" => 'done: {
            'usage: {
                let Some(args) = args else {
                    break 'usage;
                };

                let (subcommand, args) = args
                    .split_once(' ')
                    .map_or_else(|| (args, None), |(a, b)| (a, Some(b)));

                match subcommand {
                    "install" => {
                        let Some((type_, resource, version, metadata)) = args.and_then(|args| {
                            let mut iter = args.split(' ');
                            let type_ = iter.next()?;
                            let resource = iter.next()?;
                            let version = iter.next();
                            let metadata = iter.next()?.as_bytes().to_vec();

                            Some((type_, resource, version, metadata))
                        }) else {
                            println!(
                                "{IND} Usage: application install <\"url\"|\"file\"> <resource> [version] <metadata>"
                            );
                            break 'done;
                        };

                        let Ok(version) = version.map(str::parse).transpose() else {
                            println!("{IND} Invalid version: {version:?}");
                            break 'done;
                        };

                        let application_id = match type_ {
                            "url" => {
                                let Ok(url) = resource.parse() else {
                                    println!("{IND} Invalid URL: {resource}");
                                    break 'done;
                                };

                                println!("{IND} Downloading application..");

                                node.ctx_manager
                                    .install_application_from_url(url, version, Vec::new())
                                    .await?
                            }
                            "file" => {
                                let path = Utf8PathBuf::from(resource);

                                node.ctx_manager
                                    .install_application_from_path(path, version, metadata)
                                    .await?
                            }
                            unknown => {
                                println!("{IND} Unknown resource type: `{unknown}`");
                                break 'done;
                            }
                        };

                        println!("{IND} Installed application: {application_id}");
                    }
                    "ls" => {
                        println!(
                            "{IND} {c1:44} | {c2:44} | {c3:12} | Source",
                            c1 = "Application ID",
                            c2 = "Blob ID",
                            c3 = "Version",
                        );

                        for application in node.ctx_manager.list_installed_applications()? {
                            let entry = format!(
                                "{c1:44} | {c2:44} | {c3:>12} | {c4}",
                                c1 = application.id,
                                c2 = application.blob,
                                c3 = application
                                    .version
                                    .map(|ver| ver.to_string())
                                    .unwrap_or_default(),
                                c4 = application.source
                            );
                            for line in entry.lines() {
                                println!("{IND} {}", line.cyan());
                            }
                        }
                    }
                    // todo! a "show" subcommand should help keep "ls" compact
                    unknown => {
                        println!("{IND} Unknown command: `{unknown}`");
                        break 'usage;
                    }
                }

                break 'done;
            };
            println!("{IND} Usage: application [ls|install]");
        }
        "context" => 'done: {
            'usage: {
                let Some(args) = args else {
                    break 'usage;
                };

                let (subcommand, args) = args
                    .split_once(' ')
                    .map_or_else(|| (args, None), |(a, b)| (a, Some(b)));

                match subcommand {
                    "ls" => {
                        println!(
                            "{IND} {c1:44} | {c2:64} | Last Transaction",
                            c1 = "Context ID",
                            c2 = "Application ID",
                        );

                        let handle = node.store.handle();

                        for (k, v) in handle.iter::<ContextMetaKey>()?.entries() {
                            let (k, v) = (k?, v?);
                            let (cx, app_id, last_tx) = (
                                k.context_id(),
                                v.application.application_id(),
                                v.last_transaction_hash,
                            );
                            let entry = format!(
                                "{c1:44} | {c2:44} | {c3}",
                                c1 = cx,
                                c2 = app_id,
                                c3 = Hash::from(last_tx)
                            );
                            for line in entry.lines() {
                                println!("{IND} {}", line.cyan());
                            }
                        }
                    }
                    "join" => {
                        let Some((context_id, private_key)) = args.and_then(|args| {
                            let mut iter = args.split(' ');
                            let context = iter.next()?;
                            let private_key = iter.next();

                            Some((context, private_key))
                        }) else {
                            println!("{IND} Usage: context join <context_id> [private_key]");
                            break 'done;
                        };

                        let Ok(context_id) = context_id.parse() else {
                            println!("{IND} Invalid context ID: {context_id}");
                            break 'done;
                        };

                        join_context(&node.ctx_manager, context_id, private_key).await?;

                        println!(
                            "{IND} Joined context {context_id}, waiting for catchup to complete..."
                        );
                    }
                    "leave" => {
                        let Some(context_id) = args else {
                            println!("{IND} Usage: context leave <context_id>");
                            break 'done;
                        };

                        let Ok(context_id) = context_id.parse() else {
                            println!("{IND} Invalid context ID: {context_id}");
                            break 'done;
                        };

                        let _ = node.ctx_manager.delete_context(&context_id).await?;

                        println!("{IND} Left context {context_id}");
                    }
                    "create" => {
                        let Some((application_id, private_key)) = args.and_then(|args| {
                            let mut iter = args.split(' ');
                            let application = iter.next()?;
                            let private_key = iter.next();
                            Some((application, private_key))
                        }) else {
                            println!("{IND} Usage: context create <application_id> [private_key]");
                            break 'done;
                        };

                        let Ok(application_id) = application_id.parse() else {
                            println!("{IND} Invalid application ID: {application_id}");
                            break 'done;
                        };

                        let context_create_result =
                            create_context(&node.ctx_manager, application_id, private_key).await?;

                        println!("{IND} Created context {}", context_create_result.context.id);
                    }
                    "delete" => {
                        let Some(context_id) = args else {
                            println!("{IND} Usage: context delete <context_id>");
                            break 'done;
                        };

                        let Ok(context_id) = context_id.parse() else {
                            println!("{IND} Invalid context ID: {context_id}");
                            break 'done;
                        };

                        let _ = node.ctx_manager.delete_context(&context_id).await?;

                        println!("{IND} Deleted context {context_id}");
                    }
                    "transactions" => {
                        let Some(context_id) = args else {
                            println!("{IND} Usage: context transactions <context_id>");
                            break 'done;
                        };

                        let Ok(context_id) = context_id.parse() else {
                            println!("{IND} Invalid context ID: {context_id}");
                            break 'done;
                        };

                        let handle = node.store.handle();

                        let mut iter = handle.iter::<ContextTransactionKey>()?;

                        let first = 'first: {
                            let Some(k) = iter
                                .seek(ContextTransactionKey::new(context_id, [0; 32]))
                                .transpose()
                            else {
                                break 'first None;
                            };

                            Some((k, iter.read()))
                        };

                        println!("{IND} {c1:44} | {c2:44}", c1 = "Hash", c2 = "Prior Hash");

                        for (k, v) in first.into_iter().chain(iter.entries()) {
                            let (k, v) = (k?, v?);
                            let entry = format!(
                                "{c1:44} | {c2}",
                                c1 = Hash::from(k.transaction_id()),
                                c2 = Hash::from(v.prior_hash),
                            );
                            for line in entry.lines() {
                                println!("{IND} {}", line.cyan());
                            }
                        }
                    }
                    "state" => {
                        let Some(context_id) = args else {
                            println!("{IND} Usage: context state <context_id>");
                            break 'done;
                        };

                        let Ok(context_id) = context_id.parse() else {
                            println!("{IND} Invalid context ID: {context_id}");
                            break 'done;
                        };

                        let handle = node.store.handle();

                        println!("{IND} {c1:44} | {c2:44}", c1 = "State Key", c2 = "Value");

                        let mut iter = handle.iter::<ContextStateKey>()?;

                        // let first = 'first: {
                        //     let Some(k) = iter
                        //         .seek(ContextStateKey::new(context_id, [0; 32]))
                        //         .transpose()
                        //     else {
                        //         break 'first None;
                        //     };

                        //     Some((k, iter.read()))
                        //                   ^^^^~ ContextState<'a> lends the `iter`, while `.entries()` attempts to mutate it
                        // };

                        for (k, v) in iter.entries() {
                            let (k, v) = (k?, v?);
                            if k.context_id() != context_id {
                                // todo! revisit this when DBIter::seek no longer returns
                                // todo! the sought item, you have to call next(), read()
                                continue;
                            }
                            let entry = format!(
                                "{c1:44} | {c2:?}",
                                c1 = Hash::from(k.state_key()),
                                c2 = v.value,
                            );
                            for line in entry.lines() {
                                println!("{IND} {}", line.cyan());
                            }
                        }
                    }
                    unknown => {
                        println!("{IND} Unknown command: `{unknown}`");
                        break 'usage;
                    }
                }

                break 'done;
            };
            println!("{IND} Usage: context [ls|join|leave|create|delete|state] [args]");
        }
        unknown => {
            println!("{IND} Unknown command: `{unknown}`");
            println!("{IND} Usage: [call|peers|pool|gc|store|context|application] [args]");
        }
    }

    Ok(())
}

#[allow(clippy::multiple_inherent_impl)]
impl Node {
    #[must_use]
    pub fn new(
        config: &NodeConfig,
        network_client: NetworkClient,
        node_events: broadcast::Sender<NodeEvent>,
        ctx_manager: ContextManager,
        store: Store,
    ) -> Self {
        Self {
            id: config.identity.public().to_peer_id(),
            typ: config.node_type,
            store,
            tx_pool: TransactionPool::default(),
            ctx_manager,
            network_client,
            node_events,
            // --
            nonce: 0,
        }
    }

    pub async fn handle_event(&mut self, event: NetworkEvent) -> EyreResult<()> {
        match event {
            NetworkEvent::Subscribed {
                peer_id: their_peer_id,
                topic: topic_hash,
            } => {
                if let Err(err) = self.handle_subscribed(their_peer_id, &topic_hash) {
                    error!(?err, "Failed to handle subscribed event");
                }
            }
            NetworkEvent::Message { message, .. } => {
                if let Err(err) = self.handle_message(message).await {
                    error!(?err, "Failed to handle message event");
                }
            }
            NetworkEvent::ListeningOn { address, .. } => {
                info!("Listening on: {}", address);
            }
            NetworkEvent::StreamOpened { peer_id, stream } => {
                info!("Stream opened from peer: {}", peer_id);

                if let Err(err) = self.handle_opened_stream(stream).await {
                    error!(?err, "Failed to handle stream");
                }

                info!("Stream closed from peer: {:?}", peer_id);
            }
            _ => error!("Unhandled event: {:?}", event),
        }

        Ok(())
    }

    fn handle_subscribed(&self, their_peer_id: PeerId, topic_hash: &TopicHash) -> EyreResult<()> {
        let Ok(context_id) = topic_hash.as_str().parse() else {
            // bail!(
            //     "Failed to parse topic hash '{}' into context ID",
            //     topic_hash
            // );
            return Ok(());
        };

        let handle = self.store.handle();

        if !handle.has(&ContextMetaKey::new(context_id))? {
            debug!(
                %context_id,
                %their_peer_id,
                "Observed subscription to unknown context, ignoring.."
            );
            return Ok(());
        };

        info!("{} joined the session.", their_peer_id.cyan());
        drop(
            self.node_events
                .send(NodeEvent::Application(ApplicationEvent::new(
                    context_id,
                    ApplicationEventPayload::PeerJoined(PeerJoinedPayload::new(their_peer_id)),
                ))),
        );

        Ok(())
    }

    async fn handle_message(&mut self, message: Message) -> EyreResult<()> {
        let Some(source) = message.source else {
            warn!(?message, "Received message without source");
            return Ok(());
        };

        match from_json_slice(&message.data)? {
            PeerAction::Transaction(transaction) => {
                debug!(?transaction, %source, "Received transaction");

                let handle = self.store.handle();

                let ctx_meta_key = ContextMetaKey::new(transaction.context_id);
                let prior_transaction_key = ContextTransactionKey::new(
                    transaction.context_id,
                    transaction.prior_hash.into(),
                );

                let transaction_hash = self.tx_pool.insert(source, transaction.clone(), None)?;

                if !handle.has(&ctx_meta_key)?
                    || (transaction.prior_hash != Hash::default()
                        && !handle.has(&prior_transaction_key)?
                        && !self.typ.is_coordinator())
                {
                    info!(context_id=%transaction.context_id, %source, "Attempting to perform tx triggered catchup");

                    self.perform_catchup(transaction.context_id, source).await?;

                    let _ = self
                        .ctx_manager
                        .clear_context_pending_catchup(&transaction.context_id)
                        .await;

                    info!(context_id=%transaction.context_id, %source, "Tx triggered catchup successfully finished");
                };

                let Some(context) = self.ctx_manager.get_context(&transaction.context_id)? else {
                    bail!("Context '{}' not found", transaction.context_id);
                };

                if self.typ.is_coordinator() {
                    let Some(pool_entry) = self.tx_pool.remove(&transaction_hash) else {
                        return Ok(());
                    };

                    let _ = self
                        .validate_pending_transaction(
                            context,
                            pool_entry.transaction,
                            transaction_hash,
                        )
                        .await?;
                }
            }
            PeerAction::TransactionConfirmation(confirmation) => {
                debug!(?confirmation, %source, "Received transaction confirmation");
                // todo! ensure this was only sent by a coordinator

                let Some(TransactionPoolEntry {
                    transaction,
                    outcome_sender,
                    ..
                }) = self.tx_pool.remove(&confirmation.transaction_hash)
                else {
                    return Ok(());
                };

                let outcome_result = self
                    .execute_in_context(confirmation.transaction_hash, transaction)
                    .await;

                if let Some(outcome_sender) = outcome_sender {
                    drop(outcome_sender.send(outcome_result));
                }
            }
            PeerAction::TransactionRejection(rejection) => {
                debug!(?rejection, %source, "Received transaction rejection");
                // todo! ensure this was only sent by a coordinator

                let _ = self.reject_from_pool(rejection.transaction_hash);

                info!(context_id=%rejection.context_id, %source, "Attempting to perform rejection triggered catchup");

                self.perform_catchup(rejection.context_id, source).await?;

                let _ = self
                    .ctx_manager
                    .clear_context_pending_catchup(&rejection.context_id)
                    .await;

                info!(context_id=%rejection.context_id, %source, "Rejection triggered catchup successfully finished");
            }
        }

        Ok(())
    }

    async fn validate_pending_transaction(
        &mut self,
        context: Context,
        transaction: Transaction,
        transaction_hash: Hash,
    ) -> EyreResult<bool> {
        if context.last_transaction_hash == transaction.prior_hash {
            self.nonce = self.nonce.saturating_add(1);

            self.push_action(
                transaction.context_id,
                PeerAction::TransactionConfirmation(TransactionConfirmation {
                    context_id: transaction.context_id,
                    nonce: self.nonce,
                    transaction_hash,
                    // todo! proper confirmation hash
                    confirmation_hash: transaction_hash,
                }),
            )
            .await?;

            self.persist_transaction(&context, transaction.clone(), transaction_hash)?;

            Ok(true)
        } else {
            self.push_action(
                transaction.context_id,
                PeerAction::TransactionRejection(TransactionRejection {
                    context_id: transaction.context_id,
                    transaction_hash,
                }),
            )
            .await?;

            Ok(false)
        }
    }

    async fn push_action(&self, context_id: ContextId, action: PeerAction) -> EyreResult<()> {
        drop(
            self.network_client
                .publish(TopicHash::from_raw(context_id), to_json_vec(&action)?)
                .await?,
        );

        Ok(())
    }

    pub async fn handle_call(
        &mut self,
        context_id: ContextId,
        method: String,
        payload: Vec<u8>,
        write: bool,
        executor_public_key: [u8; 32],
        outcome_sender: oneshot::Sender<Result<Outcome, CallError>>,
    ) {
        let Ok(Some(context)) = self.ctx_manager.get_context(&context_id) else {
            drop(outcome_sender.send(Err(CallError::ContextNotFound { context_id })));
            return;
        };

        if write {
            let (inner_outcome_sender, inner_outcome_receiver) = oneshot::channel();

            if let Err(err) = self
                .call_mutate(
                    context,
                    method,
                    payload,
                    executor_public_key,
                    inner_outcome_sender,
                )
                .await
            {
                drop(outcome_sender.send(Err(CallError::Mutate(err))));
                return;
            }

            drop(spawn(async move {
                match inner_outcome_receiver.await {
                    Ok(outcome) => match outcome {
                        Ok(outcome) => {
                            drop(outcome_sender.send(Ok(outcome)));
                        }
                        Err(err) => {
                            drop(outcome_sender.send(Err(CallError::Mutate(err))));
                        }
                    },
                    Err(err) => {
                        error!("Failed to receive inner outcome of a transaction: {}", err);
                        drop(
                            outcome_sender
                                .send(Err(CallError::Mutate(MutateCallError::InternalError))),
                        );
                    }
                }
            }));
        } else {
            match self
                .call_query(context, method, payload, executor_public_key)
                .await
            {
                Ok(outcome) => {
                    drop(outcome_sender.send(Ok(outcome)));
                }
                Err(err) => {
                    drop(outcome_sender.send(Err(CallError::Query(err))));
                }
            };
        }
    }

    async fn call_query(
        &mut self,
        context: Context,
        method: String,
        payload: Vec<u8>,
        executor_public_key: [u8; 32],
    ) -> Result<Outcome, QueryCallError> {
        if !self
            .ctx_manager
            .is_application_installed(&context.application_id)
            .unwrap_or_default()
        {
            return Err(QueryCallError::ApplicationNotInstalled {
                application_id: context.application_id,
            });
        }

        self.execute(context, None, method, payload, executor_public_key)
            .await
            .map_err(|e| {
                error!(%e,"Failed to execute query call.");
                QueryCallError::InternalError
            })
    }

    async fn call_mutate(
        &mut self,
        context: Context,
        method: String,
        payload: Vec<u8>,
        executor_public_key: [u8; 32],
        outcome_sender: oneshot::Sender<Result<Outcome, MutateCallError>>,
    ) -> Result<Hash, MutateCallError> {
        if self.typ.is_coordinator() {
            return Err(MutateCallError::InvalidNodeType {
                node_type: self.typ,
            });
        }

        if !self
            .ctx_manager
            .is_application_installed(&context.application_id)
            .unwrap_or_default()
        {
            return Err(MutateCallError::ApplicationNotInstalled {
                application_id: context.application_id,
            });
        }

        if self
            .network_client
            .mesh_peer_count(TopicHash::from_raw(context.id))
            .await
            == 0
        {
            return Err(MutateCallError::NoConnectedPeers);
        }

        let transaction = Transaction::new(
            context.id,
            method,
            payload,
            context.last_transaction_hash,
            executor_public_key,
        );

        self.push_action(context.id, PeerAction::Transaction(transaction.clone()))
            .await
            .map_err(|err| {
                error!(%err, "Failed to push transaction over the network.");
                MutateCallError::InternalError
            })?;

        let tx_hash = self
            .tx_pool
            .insert(self.id, transaction, Some(outcome_sender))
            .map_err(|err| {
                error!(%err, "Failed to insert transaction into the pool.");
                MutateCallError::InternalError
            })?;

        Ok(tx_hash)
    }

    async fn execute_in_context(
        &mut self,
        transaction_hash: Hash,
        transaction: Transaction,
    ) -> Result<Outcome, MutateCallError> {
        let Some(context) = self
            .ctx_manager
            .get_context(&transaction.context_id)
            .map_err(|e| {
                error!(%e, "Failed to get context");
                MutateCallError::InternalError
            })?
        else {
            error!(%transaction.context_id, "Context not found");
            return Err(MutateCallError::InternalError);
        };

        if context.last_transaction_hash != transaction.prior_hash {
            error!(
                context_id=%transaction.context_id,
                %transaction_hash,
                prior_hash=%transaction.prior_hash,
                "Transaction from the pool doesn't build on last transaction",
            );
            return Err(MutateCallError::TransactionRejected);
        }

        let outcome = self
            .execute_transaction(context, transaction, transaction_hash)
            .await
            .map_err(|e| {
                error!(%e, "Failed to execute transaction");
                MutateCallError::InternalError
            })?;

        Ok(outcome)
    }

    async fn execute_transaction(
        &mut self,
        context: Context,
        transaction: Transaction,
        hash: Hash,
    ) -> EyreResult<Outcome> {
        let outcome = self
            .execute(
                context.clone(),
                Some(hash),
                transaction.method.clone(),
                transaction.payload.clone(),
                transaction.executor_public_key,
            )
            .await?;

        self.persist_transaction(&context, transaction, hash)?;

        Ok(outcome)
    }

    fn reject_from_pool(&mut self, hash: Hash) -> Option<()> {
        let TransactionPoolEntry { outcome_sender, .. } = self.tx_pool.remove(&hash)?;

        if let Some(sender) = outcome_sender {
            drop(sender.send(Err(MutateCallError::TransactionRejected)));
        }

        Some(())
    }

    fn persist_transaction(
        &self,
        context: &Context,
        transaction: Transaction,
        hash: Hash,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();

        handle.put(
            &ContextTransactionKey::new(context.id, hash.into()),
            &ContextTransaction::new(
                transaction.method.into(),
                transaction.payload.into(),
                *transaction.prior_hash,
                transaction.executor_public_key,
            ),
        )?;

        handle.put(
            &ContextMetaKey::new(context.id),
            &ContextMeta::new(
                ApplicationMetaKey::new(context.application_id),
                *hash.as_bytes(),
            ),
        )?;

        Ok(())
    }

    pub async fn execute(
        &mut self,
        context: Context,
        hash: Option<Hash>,
        method: String,
        payload: Vec<u8>,
        executor_public_key: [u8; 32],
    ) -> EyreResult<Outcome> {
        let mut storage = match hash {
            Some(_) => RuntimeCompatStore::temporal(&mut self.store, context.id),
            None => RuntimeCompatStore::read_only(&self.store, context.id),
        };

        let Some(blob) = self
            .ctx_manager
            .load_application_blob(&context.application_id)
            .await?
        else {
            bail!(
                "fatal error: missing blob for application `{}`",
                context.application_id
            );
        };

        let outcome = calimero_runtime::run(
            &blob,
            &method,
            VMContext::new(payload, executor_public_key),
            &mut storage,
            &get_runtime_limits()?,
        )?;

        if let Some(hash) = hash {
            assert!(storage.commit()?, "do we have a non-temporal store?");

            // todo! return an error to the caller if the method did not write to storage
            // todo! debate: when we switch to optimistic execution
            // todo! we won't have query vs. mutate methods anymore, so this shouldn't matter

            drop(
                self.node_events
                    .send(NodeEvent::Application(ApplicationEvent::new(
                        context.id,
                        ApplicationEventPayload::TransactionExecuted(
                            ExecutedTransactionPayload::new(hash),
                        ),
                    ))),
            );
        }

        drop(
            self.node_events
                .send(NodeEvent::Application(ApplicationEvent::new(
                    context.id,
                    ApplicationEventPayload::OutcomeEvent(OutcomeEventPayload::new(
                        outcome
                            .events
                            .iter()
                            .map(|e| OutcomeEvent::new(e.kind.clone(), e.data.clone()))
                            .collect(),
                    )),
                ))),
        );

        Ok(outcome)
    }
}

// TODO: move this into the config
// TODO: also this would be nice to have global default with per application customization
fn get_runtime_limits() -> EyreResult<VMLimits> {
    Ok(VMLimits::new(
        /*max_stack_size:*/ 200 << 10, // 200 KiB
        /*max_memory_pages:*/ 1 << 10, // 1 KiB
        /*max_registers:*/ 100,
        /*max_register_size:*/ (100 << 20).validate()?, // 100 MiB
        /*max_registers_capacity:*/ 1 << 30, // 1 GiB
        /*max_logs:*/ 100,
        /*max_log_size:*/ 16 << 10, // 16 KiB
        /*max_events:*/ 100,
        /*max_event_kind_size:*/ 100,
        /*max_event_data_size:*/ 16 << 10, // 16 KiB
        /*max_storage_key_size:*/ (1 << 20).try_into()?, // 1 MiB
        /*max_storage_value_size:*/
        (10 << 20).try_into()?, // 10 MiB
                                // can_write: writes, // todo!
    ))
}
