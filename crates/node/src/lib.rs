use calimero_runtime::logic::VMLimits;
use calimero_runtime::Constraint;
use calimero_store::Store;
use libp2p::gossipsub::TopicHash;
use libp2p::identity;
use owo_colors::OwoColorize;
use tokio::io::AsyncBufReadExt;
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{error, info};

pub mod config;
pub mod temporal_runtime_store;
pub mod transaction_pool;
pub mod types;

type BoxedFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T>>>;

#[derive(Debug)]
pub struct NodeConfig {
    pub home: camino::Utf8PathBuf,
    pub identity: identity::Keypair,
    pub node_type: calimero_primitives::types::NodeType,
    pub application: calimero_application::config::ApplicationConfig,
    pub network: calimero_network::config::NetworkConfig,
    pub server: calimero_server::config::ServerConfig,
    pub store: calimero_store::config::StoreConfig,
}

pub struct Node {
    id: calimero_network::types::PeerId,
    typ: calimero_primitives::types::NodeType,
    store: calimero_store::Store,
    tx_pool: transaction_pool::TransactionPool,
    application_manager: calimero_application::ApplicationManager,
    network_client: calimero_network::client::NetworkClient,
    node_events: broadcast::Sender<calimero_primitives::events::NodeEvent>,
    // --
    nonce: u64,
    last_tx: calimero_primitives::hash::Hash,
}

pub async fn start(config: NodeConfig) -> eyre::Result<()> {
    let peer_id = config.identity.public().to_peer_id();

    info!("Peer ID: {}", peer_id);

    let (node_events, _) = broadcast::channel(32);

    let (network_client, mut network_events) = calimero_network::run(&config.network).await?;

    let application_manager =
        calimero_application::start_manager(&config.application, network_client.clone()).await?;

    let store = calimero_store::Store::open(&config.store)?;

    let mut node = Node::new(
        &config,
        network_client.clone(),
        node_events.clone(),
        application_manager.clone(),
        store.clone(),
    );

    let (server_sender, mut server_receiver) = mpsc::channel(32);

    let mut server = Box::pin(calimero_server::start(
        config.server,
        server_sender,
        application_manager,
        node_events,
        store,
    )) as BoxedFuture<eyre::Result<()>>;

    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();

    loop {
        tokio::select! {
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
                server = Box::pin(std::future::pending());
                continue;
            }
            Some((application_id, method, payload, write, tx)) = server_receiver.recv() => {
                if write {
                    if let Err(err) = node.call_mut(application_id, method, payload, tx).await {
                        error!("Failed to send transaction: {}", err);
                    }
                } else {
                    match node.call(application_id, method, payload).await {
                        Ok(outcome) => {
                            let _ = tx.send(outcome);
                        },
                        Err(err) => error!("Failed to execute transaction: {}", err)
                    };
                }
            }
        }
    }

    Ok(())
}

async fn handle_line(node: &mut Node, line: String) -> eyre::Result<()> {
    let (command, args) = match line.split_once(' ') {
        Some((method, payload)) => (method, Some(payload)),
        None => (line.as_str(), None),
    };

    #[allow(non_snake_case)]
    let IND = " │".yellow();

    // TODO: should be replaced with RPC endpoints
    match command {
        "call" => {
            if let Some((application_id, args)) = args.and_then(|args| args.split_once(' ')) {
                let (method, payload) = args.split_once(' ').unwrap_or_else(|| (args, "{}"));

                match serde_json::from_str::<serde_json::Value>(payload) {
                    Ok(_) => {
                        let (tx, rx) = oneshot::channel();

                        let tx_hash = match node
                            .call_mut(
                                application_id.to_owned().into(),
                                method.to_owned(),
                                payload.as_bytes().to_owned(),
                                tx,
                            )
                            .await
                        {
                            Ok(tx_hash) => tx_hash,
                            Err(e) => {
                                println!("{IND} Failed to send transaction: {}", e);
                                return Ok(());
                            }
                        };

                        println!("{IND} Scheduled Transaction! {:?}", tx_hash);

                        tokio::spawn(async move {
                            if let Ok(outcome) = rx.await {
                                println!("{IND} {:?}", tx_hash);

                                match outcome.returns {
                                    Ok(result) => match result {
                                        Some(result) => {
                                            println!("{IND}   Return Value:");
                                            let result = if let Ok(value) =
                                                serde_json::from_slice::<serde_json::Value>(&result)
                                            {
                                                format!(
                                                    "(json): {}",
                                                    format!("{:#}", value)
                                                        .lines()
                                                        .map(|line| line.cyan().to_string())
                                                        .collect::<Vec<_>>()
                                                        .join("\n")
                                                )
                                            } else {
                                                format!("(raw): {:?}", result.cyan())
                                            };

                                            for line in result.lines() {
                                                println!("{IND}     > {}", line);
                                            }
                                        }
                                        None => println!("{IND}   (No return value)"),
                                    },
                                    Err(err) => {
                                        let err = format!("{:#?}", err);

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
                        });
                    }
                    Err(e) => {
                        println!("{IND} Failed to parse payload: {}", e);
                    }
                }
            } else {
                println!("{IND} Usage: call <Method> <JSON Payload>");
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
                node.tx_pool = transaction_pool::TransactionPool::default();
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
                let payload = if let Ok(value) =
                    serde_json::from_slice::<serde_json::Value>(&entry.transaction.payload)
                {
                    format!(
                        "(json): {}",
                        format!("{:#}", value)
                            .lines()
                            .map(|line| line.cyan().to_string())
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                } else {
                    format!("(raw): {:?}", entry.transaction.payload.cyan())
                };

                for line in payload.lines() {
                    println!("{IND}       > {}", line);
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
                if node.application_manager.is_application_installed(
                    &calimero_primitives::application::ApplicationId(topic.clone().into_string()),
                ) {
                    println!(
                        "{IND} Peers (Session) for Topic {}: {:#?}",
                        topic.clone(),
                        node.network_client.mesh_peer_count(topic).await.cyan()
                    );
                }
            }
        }
        "store" => {
            let state = format!("{:#?}", node.store.get(&b"STATE".to_vec()));
            for line in state.lines() {
                println!("{IND} {}", line.cyan());
            }
        }
        unknown => {
            println!("{IND} Unknown command: `{}`", unknown);
            println!("{IND} Usage: [call|peers|pool|gc|store] [args]")
        }
    }

    Ok(())
}

impl Node {
    pub fn new(
        config: &NodeConfig,
        network_client: calimero_network::client::NetworkClient,
        node_events: broadcast::Sender<calimero_primitives::events::NodeEvent>,
        application_manager: calimero_application::ApplicationManager,
        store: Store,
    ) -> Self {
        Self {
            id: config.identity.public().to_peer_id(),
            typ: config.node_type,
            store,
            tx_pool: transaction_pool::TransactionPool::default(),
            application_manager,
            network_client,
            node_events,
            // --
            nonce: 0,
            last_tx: calimero_primitives::hash::Hash::default(),
        }
    }

    pub async fn handle_event(
        &mut self,
        event: calimero_network::types::NetworkEvent,
    ) -> eyre::Result<()> {
        match event {
            calimero_network::types::NetworkEvent::Subscribed {
                peer_id: their_peer_id,
                topic: topic_hash,
            } => {
                if self.application_manager.is_application_installed(
                    &calimero_primitives::application::ApplicationId(
                        topic_hash.clone().into_string(),
                    ),
                ) {
                    info!("{} joined the session.", their_peer_id.cyan());
                    let _ =
                        self.node_events
                            .send(calimero_primitives::events::NodeEvent::Application(
                            calimero_primitives::events::ApplicationEvent {
                                application_id: calimero_primitives::application::ApplicationId(
                                    topic_hash.into_string().clone(),
                                ),
                                payload:
                                    calimero_primitives::events::ApplicationEventPayload::PeerJoined(
                                        calimero_primitives::events::PeerJoinedPayload {
                                            peer_id: their_peer_id,
                                        },
                                    ),
                            },
                        ));
                }
            }
            calimero_network::types::NetworkEvent::Message { message, .. } => {
                let Some(source) = message.source else {
                    return Ok(());
                };
                match serde_json::from_slice(&message.data)? {
                    types::PeerAction::Transaction(transaction) => {
                        let transaction_hash =
                            self.tx_pool.insert(source, transaction.clone(), None)?;

                        if self.typ.is_coordinator() {
                            self.nonce += 1;

                            self.push_action(
                                transaction.application_id.clone(),
                                types::PeerAction::TransactionConfirmation(
                                    types::TransactionConfirmation {
                                        application_id: transaction.application_id.clone(),
                                        nonce: self.nonce,
                                        transaction_hash,
                                        // todo! proper confirmation hash
                                        confirmation_hash: transaction_hash,
                                    },
                                ),
                            )
                            .await?;

                            self.tx_pool.remove(&transaction_hash);
                        }
                    }
                    types::PeerAction::TransactionConfirmation(confirmation) => {
                        // todo! ensure this was only sent by a coordinator
                        self.execute_in_pool(
                            confirmation.application_id,
                            confirmation.transaction_hash,
                        )
                        .await?;
                    }
                    message => error!("Unhandled PeerAction: {:?}", message),
                }
            }
            calimero_network::types::NetworkEvent::ListeningOn { address, .. } => {
                info!("Listening on: {}", address);
            }
        }

        Ok(())
    }

    pub async fn push_action(
        &mut self,
        application_id: calimero_primitives::application::ApplicationId,
        action: types::PeerAction,
    ) -> eyre::Result<()> {
        self.network_client
            .publish(
                TopicHash::from_raw(application_id.to_string()),
                serde_json::to_vec(&action)?,
            )
            .await
            .expect("Failed to publish message.");

        Ok(())
    }

    pub async fn call(
        &mut self,
        application_id: calimero_primitives::application::ApplicationId,
        method: String,
        payload: Vec<u8>,
    ) -> eyre::Result<calimero_runtime::logic::Outcome> {
        if !self
            .application_manager
            .is_application_installed(&application_id)
        {
            eyre::bail!("Application is not installed.");
        }

        self.execute(application_id, None, method, payload).await
    }

    pub async fn call_mut(
        &mut self,
        application_id: calimero_primitives::application::ApplicationId,
        method: String,
        payload: Vec<u8>,
        tx: oneshot::Sender<calimero_runtime::logic::Outcome>,
    ) -> eyre::Result<calimero_primitives::hash::Hash> {
        if self.typ.is_coordinator() {
            eyre::bail!("Coordinator can not create transactions!");
        }

        if !self
            .application_manager
            .is_application_installed(&application_id)
        {
            eyre::bail!("Application is not installed.");
        }

        if self
            .network_client
            .mesh_peer_count(TopicHash::from_raw(application_id.clone().to_string()))
            .await
            == 0
        {
            eyre::bail!("No connected peers to send message to.");
        }

        let transaction = calimero_primitives::transaction::Transaction {
            application_id: application_id.clone(),
            method,
            payload,
            prior_hash: self.last_tx,
        };

        let tx_hash = self
            .tx_pool
            .insert(self.id, transaction.clone(), Some(tx))?;

        // todo! consider including the outcome hash in the transaction
        self.push_action(
            application_id.clone(),
            types::PeerAction::Transaction(transaction),
        )
        .await?;

        self.last_tx = tx_hash;

        Ok(tx_hash)
    }

    async fn execute_in_pool(
        &mut self,
        application_id: calimero_primitives::application::ApplicationId,
        hash: calimero_primitives::hash::Hash,
    ) -> eyre::Result<Option<()>> {
        let Some(transaction_pool::TransactionPoolEntry {
            transaction,
            outcome_sender,
            ..
        }) = self.tx_pool.remove(&hash)
        else {
            return Ok(None);
        };

        let outcome = self
            .execute(
                application_id,
                Some(hash),
                transaction.method,
                transaction.payload,
            )
            .await?;

        if let Some(sender) = outcome_sender {
            let _ = sender.send(outcome);
        }

        Ok(Some(()))
    }

    async fn execute(
        &mut self,
        application_id: calimero_primitives::application::ApplicationId,
        hash: Option<calimero_primitives::hash::Hash>,
        method: String,
        payload: Vec<u8>,
    ) -> eyre::Result<calimero_runtime::logic::Outcome> {
        let mut storage = match hash {
            Some(_) => temporal_runtime_store::TemporalRuntimeStore::Write(
                calimero_store::TemporalStore::new(application_id.clone(), &self.store),
            ),
            None => temporal_runtime_store::TemporalRuntimeStore::Read(
                calimero_store::ReadOnlyStore::new(application_id.clone(), &self.store),
            ),
        };

        info!(%application_id, %method, "Executing method");
        let short_application_id = application_id.as_ref().split('/').last().unwrap();
        info!(%short_application_id, "Executing method");

        let outcome = calimero_runtime::run(
            &self
                .application_manager
                .load_application_blob(&(short_application_id.to_string().into()))?,
            &method,
            calimero_runtime::logic::VMContext { input: payload },
            &mut storage,
            &get_runtime_limits()?,
        )?;

        if let (Ok(_), temporal_runtime_store::TemporalRuntimeStore::Write(storage), Some(hash)) =
            (&outcome.returns, storage, hash)
        {
            if storage.has_changes() {
                storage.commit()?;
            }
            /* else {
                todo!("return an error to the caller that the method did not write to storage")
            } */

            let _ = self
                .node_events
                .send(calimero_primitives::events::NodeEvent::Application(
                calimero_primitives::events::ApplicationEvent {
                    application_id,
                    payload:
                        calimero_primitives::events::ApplicationEventPayload::TransactionExecuted(
                            calimero_primitives::events::ExecutedTransactionPayload { hash },
                        ),
                },
            ));
        }

        Ok(outcome)
    }
}

// TODO: move this into the config
// TODO: also this would be nice to have global default with per application customization
fn get_runtime_limits() -> eyre::Result<VMLimits> {
    Ok(calimero_runtime::logic::VMLimits {
        max_stack_size: 200 << 10, // 200 KiB
        max_memory_pages: 1 << 10, // 1 KiB
        max_registers: 100,
        max_register_size: (100 << 20).validate()?, // 100 MiB
        max_registers_capacity: 1 << 30,            // 1 GiB
        max_logs: 100,
        max_log_size: 16 << 10,                      // 16 KiB
        max_storage_key_size: (1 << 20).try_into()?, // 1 MiB
        max_storage_value_size: (10 << 20).try_into()?, // 10 MiB
                                                     // can_write: writes, // todo!
    })
}
