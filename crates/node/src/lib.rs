use std::collections::BTreeMap;
use std::fs;

use calimero_runtime::Constraint;
use libp2p::identity;
use owo_colors::OwoColorize;
use tokio::io::AsyncBufReadExt;
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{error, info, warn};

pub mod config;
pub mod types;

type BoxedFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T>>>;

#[derive(Debug)]
pub struct NodeConfig {
    pub home: camino::Utf8PathBuf,
    pub app_path: camino::Utf8PathBuf,
    pub identity: identity::Keypair,
    pub node_type: calimero_primitives::types::NodeType,
    pub network: calimero_network::config::NetworkConfig,
    pub server: calimero_server::config::ServerConfig,
    pub store: calimero_store::config::StoreConfig,
}

#[derive(Debug)]
pub struct TransactionPoolEntry {
    sender: calimero_network::types::PeerId,
    transaction: calimero_primitives::transaction::Transaction,
    outcome_sender: Option<oneshot::Sender<calimero_runtime::logic::Outcome>>,
}

#[derive(Debug, Default)]
pub struct TransactionPool {
    transactions: BTreeMap<calimero_primitives::hash::Hash, TransactionPoolEntry>,
}

pub struct Node {
    id: calimero_network::types::PeerId,
    typ: calimero_primitives::types::NodeType,
    store: calimero_store::Store,
    tx_pool: TransactionPool,
    app_blob: Vec<u8>,
    app_topic: calimero_network::types::TopicHash,
    network_client: calimero_network::NetworkClient,
    node_events: broadcast::Sender<calimero_primitives::events::NodeEvent>,
    // --
    nonce: u64,
    last_tx: calimero_primitives::hash::Hash,
}

pub async fn start(config: NodeConfig) -> eyre::Result<()> {
    let peer_id = config.identity.public().to_peer_id();

    info!("Peer ID: {}", peer_id);

    let node_events = broadcast::channel(32).0;

    let (network_client, mut network_events) = calimero_network::run(&config.network).await?;

    let mut node = Node::new(&config, network_client, node_events.clone()).await?;

    let (server_sender, mut server_receiver) = mpsc::channel(32);

    let mut server = Box::pin(calimero_server::start(
        config.server,
        server_sender,
        node_events,
    )) as BoxedFuture<eyre::Result<()>>;

    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();

    loop {
        tokio::select! {
            event = network_events.recv() => {
                match event {
                    Some(event) => node.handle_event(event).await?,
                    None => break,
                }
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
            Some((method, payload, write, tx)) = server_receiver.recv() => {
                if write {
                    if let Err(err) = node.call_mut(method, payload, tx).await {
                        error!("Failed to send transaction: {}", err);
                    }
                } else {
                    match node.call(method, payload).await {
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

    match command {
        "call" => {
            if let Some(args) = args {
                let (method, payload) = args.split_once(' ').unwrap_or_else(|| (args, "{}"));
                match serde_json::from_str::<serde_json::Value>(payload) {
                    Ok(_) => {
                        let (tx, rx) = oneshot::channel();

                        let tx_hash = match node
                            .call_mut(method.to_owned(), payload.as_bytes().to_owned(), tx)
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
            let (all_peers, session_peers) = tokio::join!(
                node.network_client.peer_count(),
                node.network_client.mesh_peer_count(node.app_topic.clone()),
            );

            println!("{IND} Peers (General): {:#?}", all_peers.cyan());
            println!("{IND} Peers (Session): {:#?}", session_peers.cyan());
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
    pub async fn new(
        config: &NodeConfig,
        network_client: calimero_network::NetworkClient,
        node_events: broadcast::Sender<calimero_primitives::events::NodeEvent>,
    ) -> eyre::Result<Self> {
        let store = calimero_store::Store::open(&config.store)?;

        let tx_pool = TransactionPool::default();

        let app_blob = fs::read(&config.app_path)?;

        let app_topic = network_client
            .subscribe(calimero_network::types::IdentTopic::new(format!(
                "/calimero/experimental/app/{}",
                calimero_primitives::hash::Hash::hash(&app_blob),
            )))
            .await?
            .hash();

        Ok(Self {
            id: config.identity.public().to_peer_id(),
            typ: config.node_type,
            store,
            tx_pool,
            app_blob,
            app_topic,
            network_client,
            node_events,
            // --
            nonce: 0,
            last_tx: calimero_primitives::hash::Hash::default(),
        })
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
                if self.app_topic == topic_hash {
                    info!("{} joined the session.", their_peer_id.cyan());
                    if self.node_events.receiver_count() > 0 {
                        self.node_events.send(
                            calimero_primitives::events::NodeEvent::PeerJoined(their_peer_id),
                        )?;
                    }
                }
            }
            calimero_network::types::NetworkEvent::Message { message, .. } => {
                let Some(source) = message.source else {
                    return Ok(());
                };
                match serde_json::from_slice(&message.data)? {
                    types::PeerAction::Transaction(transaction) => {
                        let transaction_hash = self.tx_pool.insert(source, transaction, None)?;

                        if self.typ.is_coordinator() {
                            self.nonce += 1;

                            self.push_action(types::PeerAction::TransactionConfirmation(
                                types::TransactionConfirmation {
                                    nonce: self.nonce,
                                    transaction_hash,
                                    // todo! proper confirmation hash
                                    confirmation_hash: transaction_hash,
                                },
                            ))
                            .await?;

                            self.tx_pool.remove(&transaction_hash);
                        }
                    }
                    types::PeerAction::TransactionConfirmation(confirmation) => {
                        // todo! ensure this was only sent by a coordinator
                        self.execute_in_pool(confirmation.transaction_hash).await?;
                    }
                    message => error!("Unhandled PeerAction: {:?}", message),
                }
            }
            calimero_network::types::NetworkEvent::ListeningOn { address, .. } => {
                warn!("listening on not expected here: {}", address);
            }
        }

        Ok(())
    }

    pub async fn push_action(&mut self, action: types::PeerAction) -> eyre::Result<()> {
        self.network_client
            .publish(self.app_topic.clone(), serde_json::to_vec(&action)?)
            .await
            .expect("Failed to publish message.");

        Ok(())
    }

    pub async fn call(
        &mut self,
        method: String,
        payload: Vec<u8>,
    ) -> eyre::Result<calimero_runtime::logic::Outcome> {
        self.execute(method, payload, false).await
    }

    pub async fn call_mut(
        &mut self,
        method: String,
        payload: Vec<u8>,
        tx: oneshot::Sender<calimero_runtime::logic::Outcome>,
    ) -> eyre::Result<calimero_primitives::hash::Hash> {
        if self.typ.is_coordinator() {
            eyre::bail!("Coordinator can not create transactions!");
        }

        if self
            .network_client
            .mesh_peer_count(self.app_topic.clone())
            .await
            == 0
        {
            eyre::bail!("No connected peers to send message to.");
        }

        let transaction = calimero_primitives::transaction::Transaction {
            method,
            payload,
            prior_hash: self.last_tx,
        };

        let tx_hash = self
            .tx_pool
            .insert(self.id, transaction.clone(), Some(tx))?;

        // todo! consider including the outcome hash in the transaction
        self.push_action(types::PeerAction::Transaction(transaction))
            .await?;

        self.last_tx = tx_hash;

        Ok(tx_hash)
    }

    async fn execute_in_pool(
        &mut self,
        hash: calimero_primitives::hash::Hash,
    ) -> eyre::Result<Option<()>> {
        let TransactionPoolEntry {
            transaction,
            outcome_sender,
            ..
        } = match self.tx_pool.remove(&hash) {
            Some(entry) => entry,
            None => return Ok(None),
        };

        let outcome = self
            .execute(
                transaction.method.clone(),
                transaction.payload.clone(),
                true,
            )
            .await?;

        let mut status = calimero_primitives::events::TransactionExecutionStatus::Failed;
        if outcome.returns.is_ok() {
            status = calimero_primitives::events::TransactionExecutionStatus::Succeeded;
        }

        if self.node_events.receiver_count() > 0 {
            let event = calimero_primitives::events::NodeEvent::TransactionExecuted(
                status,
                transaction,
                outcome.logs.clone(),
            );
            self.node_events.send(event)?;
        }

        if let Some(sender) = outcome_sender {
            let _ = sender.send(outcome);
        }

        Ok(Some(()))
    }

    async fn execute(
        &mut self,
        method: String,
        payload: Vec<u8>,
        writes: bool,
    ) -> eyre::Result<calimero_runtime::logic::Outcome> {
        let mut storage = if writes {
            TemporalRuntimeStore::Write(calimero_store::TemporalStore::new(&self.store))
        } else {
            TemporalRuntimeStore::Read(calimero_store::ReadOnlyStore::new(&self.store))
        };

        let limits = calimero_runtime::logic::VMLimits {
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
        };

        let outcome = calimero_runtime::run(
            &self.app_blob,
            &method,
            calimero_runtime::logic::VMContext { input: payload },
            &mut storage,
            &limits,
        )?;

        if let (Ok(_), TemporalRuntimeStore::Write(storage)) = (&outcome.returns, storage) {
            if storage.has_changes() {
                storage.commit()?;
            }
            /* else {
                todo!("return an error to the caller that the method did not write to storage")
            } */
        }

        Ok(outcome)
    }
}

pub enum TemporalRuntimeStore {
    Read(calimero_store::ReadOnlyStore),
    Write(calimero_store::TemporalStore),
}

impl calimero_runtime::store::Storage for TemporalRuntimeStore {
    fn get(&self, key: &calimero_runtime::store::Key) -> Option<Vec<u8>> {
        match self {
            Self::Read(store) => store.get(key).ok().flatten(),
            Self::Write(store) => store.get(key).ok().flatten(),
        }
    }

    fn set(
        &mut self,
        key: calimero_runtime::store::Key,
        value: calimero_runtime::store::Value,
    ) -> Option<calimero_runtime::store::Value> {
        match self {
            Self::Read(_) => unimplemented!("Can not write to read-only store."),
            Self::Write(store) => store.put(key, value),
        }
    }

    fn has(&self, key: &calimero_runtime::store::Key) -> bool {
        // todo! optimize to avoid eager reads
        match self {
            Self::Read(store) => store.get(key).ok().is_some(),
            Self::Write(store) => store.get(key).ok().is_some(),
        }
    }
}

impl TransactionPool {
    fn insert(
        &mut self,
        sender: calimero_network::types::PeerId,
        transaction: calimero_primitives::transaction::Transaction,
        outcome_sender: Option<oneshot::Sender<calimero_runtime::logic::Outcome>>,
    ) -> eyre::Result<calimero_primitives::hash::Hash> {
        let transaction_hash = calimero_primitives::hash::Hash::hash_json(&transaction)
            .expect("Failed to hash transaction. This is a bug and should be reported.");

        self.transactions.insert(
            transaction_hash,
            TransactionPoolEntry {
                sender,
                transaction,
                outcome_sender: outcome_sender,
            },
        );

        Ok(transaction_hash)
    }

    fn remove(&mut self, hash: &calimero_primitives::hash::Hash) -> Option<TransactionPoolEntry> {
        self.transactions.remove(hash)
    }
}
