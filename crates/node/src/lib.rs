use calimero_primitives::events::OutcomeEvent;
use calimero_runtime::logic::VMLimits;
use calimero_runtime::Constraint;
use calimero_store::Store;
use libp2p::gossipsub::TopicHash;
use libp2p::identity;
use owo_colors::OwoColorize;
use tokio::io::AsyncBufReadExt;
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{error, info};

pub mod runtime_compat;
pub mod transaction_pool;
pub mod types;

type BoxedFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T>>>;

#[derive(Debug)]
pub struct NodeConfig {
    pub home: camino::Utf8PathBuf,
    pub identity: identity::Keypair,
    pub node_type: calimero_node_primitives::NodeType,
    pub application: calimero_context::config::ApplicationConfig,
    pub network: calimero_network::config::NetworkConfig,
    pub server: calimero_server::config::ServerConfig,
    pub store: calimero_store::config::StoreConfig,
}

pub struct Node {
    id: calimero_network::types::PeerId,
    typ: calimero_node_primitives::NodeType,
    store: calimero_store::Store,
    tx_pool: transaction_pool::TransactionPool,
    ctx_manager: calimero_context::ContextManager,
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

    let store = calimero_store::Store::open::<calimero_store::db::RocksDB>(&config.store)?;

    let ctx_manager = calimero_context::ContextManager::start(
        &config.application,
        store.clone(),
        network_client.clone(),
    )
    .await?;

    let mut node = Node::new(
        &config,
        network_client.clone(),
        node_events.clone(),
        ctx_manager.clone(),
        store.clone(),
    );

    let (server_sender, mut server_receiver) = mpsc::channel(32);

    let mut server = Box::pin(calimero_server::start(
        config.server,
        server_sender,
        ctx_manager,
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
            Some((context_id, method, payload, write, outcome_sender)) = server_receiver.recv() => {
                node.handle_call(context_id, method, payload, write, outcome_sender).await;
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
            if let Some((context_id, args)) = args.and_then(|args| args.split_once(' ')) {
                let (method, payload) = args.split_once(' ').unwrap_or_else(|| (args, "{}"));

                match serde_json::from_str::<serde_json::Value>(payload) {
                    Ok(_) => {
                        let (outcome_sender, outcome_receiver) = oneshot::channel();

                        let context_id = context_id.parse()?;

                        let Ok(Some(context)) = node.ctx_manager.get_context(&context_id) else {
                            println!("{IND} Context not found: {}", context_id);
                            return Ok(());
                        };

                        let tx_hash = match node
                            .call_mutate(
                                context,
                                method.to_owned(),
                                payload.as_bytes().to_owned(),
                                outcome_sender,
                            )
                            .await
                        {
                            Ok(tx_hash) => tx_hash,
                            Err(e) => {
                                println!("{IND} Failed to execute transaction: {}", e);
                                return Ok(());
                            }
                        };

                        println!("{IND} Scheduled Transaction! {:?}", tx_hash);

                        tokio::spawn(async move {
                            if let Ok(outcome) = outcome_receiver.await {
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
                if node.ctx_manager.is_application_installed(
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
            // todo! revisit: get specific context state
            // todo! test this

            println!(
                "{IND} {c1:44} | {c2:44} | {c3}",
                c1 = "Context ID",
                c2 = "State Key",
                c3 = "Value"
            );

            let key = calimero_store::key::ContextState::new([0; 32].into(), [0; 32].into());

            let handle = node.store.handle();

            for (k, v) in &mut handle.iter(&key)?.entries() {
                let (cx, state_key) = (k.context_id(), k.state_key());
                let sk = calimero_primitives::hash::Hash::from(state_key);
                let entry = format!("{c1:44} | {c2:44}| {c3:?}", c1 = cx, c2 = sk, c3 = v.value);
                for line in entry.lines() {
                    println!("{IND} {}", line.cyan());
                }
            }
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
                        // todo! application ID shouldn't be hex anymore
                        println!(
                            "{IND} {c1:44} | {c2:64} | {c3}",
                            c1 = "Context ID",
                            c2 = "Application ID",
                            c3 = "Last Transaction"
                        );

                        let handle = node.store.handle();

                        for (k, v) in &mut handle
                            .iter(&calimero_store::key::ContextMeta::new([0; 32].into()))?
                            .entries()
                        {
                            let (cx, app_id, last_tx) =
                                (k.context_id(), v.application_id, v.last_transaction_hash);
                            let entry = format!(
                                "{c1:44} | {c2:64} | {c3}",
                                c1 = cx,
                                c2 = app_id,
                                c3 = calimero_primitives::hash::Hash::from(last_tx)
                            );
                            for line in entry.lines() {
                                println!("{IND} {}", line.cyan());
                            }
                        }
                    }
                    "join" => {
                        let Some(context_id) = args else {
                            println!("{IND} Usage: context join <context_id>");
                            break 'done;
                        };

                        let Ok(context_id) = context_id.parse() else {
                            println!("{IND} Invalid context ID: {}", context_id);
                            break 'done;
                        };

                        node.ctx_manager.join_context(&context_id).await?;

                        println!(
                            "{IND} Joined context {}, waiting for catchup to complete..",
                            context_id
                        );
                    }
                    "leave" => {
                        let Some(context_id) = args else {
                            println!("{IND} Usage: context leave <context_id>");
                            break 'done;
                        };

                        let Ok(context_id) = context_id.parse() else {
                            println!("{IND} Invalid context ID: {}", context_id);
                            break 'done;
                        };

                        node.ctx_manager.delete_context(&context_id).await?;

                        println!("{IND} Left context {}", context_id);
                    }
                    "create" => {
                        let Some((context_id, application_id, version)) = args.and_then(|args| {
                            let mut iter = args.split(' ');
                            let context = iter.next()?;
                            let application = iter.next()?;
                            let version = iter.next()?;

                            Some((context, application, version))
                        }) else {
                            println!("{IND} Usage: context create <context_id> <application_id> <version>");
                            break 'done;
                        };

                        let Ok(context_id) = context_id.parse() else {
                            println!("{IND} Invalid context ID: {}", context_id);
                            break 'done;
                        };

                        let Ok(version) = version.parse() else {
                            println!("{IND} Invalid version: {}", version);
                            break 'done;
                        };

                        let application_id = application_id.to_owned().into();

                        println!("{IND} Downloading application..");

                        // todo! we should be able to install latest version
                        node.ctx_manager
                            .install_application(&application_id, &version)
                            .await?;

                        let context = calimero_primitives::context::Context {
                            id: context_id,
                            application_id,
                        };

                        node.ctx_manager.add_context(context).await?;

                        println!("{IND} Created context {}", context_id);
                    }
                    "delete" => {
                        let Some(context_id) = args else {
                            println!("{IND} Usage: context delete <context_id>");
                            break 'done;
                        };

                        let Ok(context_id) = context_id.parse() else {
                            println!("{IND} Invalid context ID: {}", context_id);
                            break 'done;
                        };

                        node.ctx_manager.delete_context(&context_id).await?;

                        println!("{IND} Deleted context {}", context_id);
                    }
                    "state" => {
                        let Some(context_id) = args else {
                            println!("{IND} Usage: context state <context_id>");
                            break 'done;
                        };

                        let Ok(context_id) = context_id.parse() else {
                            println!("{IND} Invalid context ID: {}", context_id);
                            break 'done;
                        };

                        let handle = node.store.handle();

                        let key =
                            calimero_store::key::ContextState::new(context_id, [0; 32].into());

                        println!("{IND} {c1:44} | {c2:44}", c1 = "State Key", c2 = "Value");

                        for (k, v) in &mut handle.iter(&key)?.entries() {
                            let entry = format!(
                                "{c1:44} | {c2:?}",
                                c1 = calimero_primitives::hash::Hash::from(k.state_key()),
                                c2 = v.value,
                            );
                            for line in entry.lines() {
                                println!("{IND} {}", line.cyan());
                            }
                        }
                    }
                    unknown => {
                        println!("{IND} Unknown command: `{}`", unknown);
                        break 'usage;
                    }
                }

                break 'done;
            };
            println!("{IND} Usage: context [ls|join|leave|create|delete|state] [args]");
        }
        unknown => {
            println!("{IND} Unknown command: `{}`", unknown);
            println!("{IND} Usage: [call|peers|pool|gc|store|context] [args]")
        }
    }

    Ok(())
}

impl Node {
    pub fn new(
        config: &NodeConfig,
        network_client: calimero_network::client::NetworkClient,
        node_events: broadcast::Sender<calimero_primitives::events::NodeEvent>,
        ctx_manager: calimero_context::ContextManager,
        store: Store,
    ) -> Self {
        Self {
            id: config.identity.public().to_peer_id(),
            typ: config.node_type,
            store,
            tx_pool: transaction_pool::TransactionPool::default(),
            ctx_manager,
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
                let Ok(context_id) = topic_hash.as_str().parse() else {
                    error!(
                        %topic_hash,
                        %their_peer_id,
                        "observed subscription to non-context topic, ignoring.."
                    );

                    return Ok(());
                };

                let Some(context) = self.ctx_manager.get_context(&context_id)? else {
                    error!(
                        %context_id,
                        %their_peer_id,
                        "observed subscription to unknown context, ignoring.."
                    );

                    return Ok(());
                };

                if self
                    .ctx_manager
                    .is_application_installed(&context.application_id)
                {
                    info!("{} joined the session.", their_peer_id.cyan());
                    let _ =
                        self.node_events
                            .send(calimero_primitives::events::NodeEvent::Application(
                            calimero_primitives::events::ApplicationEvent {
                                context_id: topic_hash.as_str().parse()?,
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
                                transaction.context_id,
                                types::PeerAction::TransactionConfirmation(
                                    types::TransactionConfirmation {
                                        context_id: transaction.context_id,
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
                            confirmation.context_id,
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
            calimero_network::types::NetworkEvent::StreamOpened { peer_id, stream } => {
                info!("Stream opened from peer: {}", peer_id);
                if let Err(err) = self.handle_stream(stream).await {
                    error!(%err, "Failed to handle stream");
                }

                info!("Stream closed from peer: {:?}", peer_id);
            }
        }

        Ok(())
    }

    pub async fn push_action(
        &mut self,
        context_id: calimero_primitives::context::ContextId,
        action: types::PeerAction,
    ) -> eyre::Result<()> {
        self.network_client
            .publish(
                TopicHash::from_raw(context_id),
                serde_json::to_vec(&action)?,
            )
            .await
            .expect("Failed to publish message.");

        Ok(())
    }

    pub async fn handle_call(
        &mut self,
        context_id: calimero_primitives::context::ContextId,
        method: String,
        payload: Vec<u8>,
        write: bool,
        outcome_sender: oneshot::Sender<
            Result<calimero_runtime::logic::Outcome, calimero_node_primitives::CallError>,
        >,
    ) {
        let Ok(Some(context)) = self.ctx_manager.get_context(&context_id) else {
            let _ =
                outcome_sender.send(Err(calimero_node_primitives::CallError::ContextNotFound {
                    context_id,
                }));
            return;
        };

        if write {
            let (inner_outcome_sender, inner_outcome_receiver) = oneshot::channel();

            if let Err(err) = self
                .call_mutate(context, method, payload, inner_outcome_sender)
                .await
            {
                let _ = outcome_sender.send(Err(calimero_node_primitives::CallError::Mutate(err)));
                return;
            }

            tokio::spawn(async move {
                match inner_outcome_receiver.await {
                    Ok(outcome) => {
                        let _ = outcome_sender.send(Ok(outcome));
                    }
                    Err(err) => {
                        error!("Failed to receive inner outcome of a transaction: {}", err);
                        let _ =
                            outcome_sender.send(Err(calimero_node_primitives::CallError::Mutate(
                                calimero_node_primitives::MutateCallError::InternalError,
                            )));
                    }
                }
            });
        } else {
            match self.call_query(context, method, payload).await {
                Ok(outcome) => {
                    let _ = outcome_sender.send(Ok(outcome));
                }
                Err(err) => {
                    let _ =
                        outcome_sender.send(Err(calimero_node_primitives::CallError::Query(err)));
                }
            };
        }
    }

    async fn call_query(
        &mut self,
        context: calimero_primitives::context::Context,
        method: String,
        payload: Vec<u8>,
    ) -> Result<calimero_runtime::logic::Outcome, calimero_node_primitives::QueryCallError> {
        if !self
            .ctx_manager
            .is_application_installed(&context.application_id)
        {
            return Err(
                calimero_node_primitives::QueryCallError::ApplicationNotInstalled {
                    application_id: context.application_id,
                },
            );
        }

        self.execute(context, None, method, payload)
            .await
            .map_err(|e| {
                error!(%e,"Failed to execute query call.");
                calimero_node_primitives::QueryCallError::InternalError
            })
    }

    async fn call_mutate(
        &mut self,
        context: calimero_primitives::context::Context,
        method: String,
        payload: Vec<u8>,
        outcome_sender: oneshot::Sender<calimero_runtime::logic::Outcome>,
    ) -> Result<calimero_primitives::hash::Hash, calimero_node_primitives::MutateCallError> {
        if self.typ.is_coordinator() {
            return Err(calimero_node_primitives::MutateCallError::InvalidNodeType {
                node_type: self.typ,
            });
        }

        if !self
            .ctx_manager
            .is_application_installed(&context.application_id)
        {
            return Err(
                calimero_node_primitives::MutateCallError::ApplicationNotInstalled {
                    application_id: context.application_id,
                },
            );
        }

        if self
            .network_client
            .mesh_peer_count(TopicHash::from_raw(context.id))
            .await
            == 0
        {
            return Err(calimero_node_primitives::MutateCallError::NoConnectedPeers);
        }

        let transaction = calimero_primitives::transaction::Transaction {
            context_id: context.id,
            method,
            payload,
            prior_hash: self.last_tx,
        };

        let tx_hash = match self
            .tx_pool
            .insert(self.id, transaction.clone(), Some(outcome_sender))
        {
            Ok(tx_hash) => tx_hash,
            Err(err) => {
                error!(%err, "Failed to insert transaction into the pool.");
                return Err(calimero_node_primitives::MutateCallError::InternalError);
            }
        };

        if let Err(err) = self
            .push_action(context.id, types::PeerAction::Transaction(transaction))
            .await
        {
            if self.tx_pool.remove(&tx_hash).is_none() {
                error!("Failed to remove just inserted transaction from the pool. This is a bug and should be reported.");
                return Err(calimero_node_primitives::MutateCallError::InternalError);
            }

            error!(%err, "Failed to push transaction over the network.");
            return Err(calimero_node_primitives::MutateCallError::InternalError);
        }

        self.last_tx = tx_hash;

        Ok(tx_hash)
    }

    async fn execute_in_pool(
        &mut self,
        context_id: calimero_primitives::context::ContextId,
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

        let Some(context) = self.ctx_manager.get_context(&context_id)? else {
            error!("Context not installed, but the transaction was in the pool.");
            return Ok(None);
        };

        let outcome = self
            .execute(
                context.clone(),
                Some(hash),
                transaction.method.clone(),
                transaction.payload.clone(),
            )
            .await?;

        let mut handle = self.store.handle();

        handle.put(
            &calimero_store::key::ContextTransaction::new(context_id, hash.into()),
            &calimero_store::types::ContextTransaction {
                method: transaction.method.into(),
                payload: transaction.payload.into(),
                prior_hash: *transaction.prior_hash,
            },
        )?;

        let key = calimero_store::key::ContextMeta::new(context_id);
        let value = calimero_store::types::ContextMeta {
            application_id: context.application_id.0.into(),
            last_transaction_hash: *hash.as_bytes(),
        };
        handle.put(&key, &value)?;

        if let Some(sender) = outcome_sender {
            let _ = sender.send(outcome);
        }

        Ok(Some(()))
    }

    async fn execute(
        &mut self,
        context: calimero_primitives::context::Context,
        hash: Option<calimero_primitives::hash::Hash>,
        method: String,
        payload: Vec<u8>,
    ) -> eyre::Result<calimero_runtime::logic::Outcome> {
        let mut storage = match hash {
            Some(_) => runtime_compat::RuntimeCompatStore::temporal(&mut self.store, context.id),
            None => runtime_compat::RuntimeCompatStore::read_only(&self.store, context.id),
        };

        let outcome = calimero_runtime::run(
            &self
                .ctx_manager
                .load_application_blob(&context.application_id)?,
            &method,
            calimero_runtime::logic::VMContext { input: payload },
            &mut storage,
            &get_runtime_limits()?,
        )?;

        if let Some(hash) = hash {
            assert!(storage.commit()?, "do we have a non-temporal store?");

            // todo! return an error to the caller if the method did not write to storage
            // todo! debate: when we switch to optimistic execution
            // todo! we won't have query vs. mutate methods anymore, so this shouldn't matter

            let _ = self
                .node_events
                .send(calimero_primitives::events::NodeEvent::Application(
                calimero_primitives::events::ApplicationEvent {
                    context_id: context.id,
                    payload:
                        calimero_primitives::events::ApplicationEventPayload::TransactionExecuted(
                            calimero_primitives::events::ExecutedTransactionPayload { hash },
                        ),
                },
            ));
        }

        let _ = self
            .node_events
            .send(calimero_primitives::events::NodeEvent::Application(
                calimero_primitives::events::ApplicationEvent {
                    context_id: context.id,
                    payload: calimero_primitives::events::ApplicationEventPayload::OutcomeEvent(
                        calimero_primitives::events::OutcomeEventPayload {
                            events: outcome
                                .events
                                .iter()
                                .map(|e| OutcomeEvent {
                                    data: e.data.clone(),
                                    kind: e.kind.clone(),
                                })
                                .collect(),
                        },
                    ),
                },
            ));

        Ok(outcome)
    }

    async fn handle_stream(
        &mut self,
        _stream: calimero_network::stream::Stream,
    ) -> eyre::Result<()> {
        // TODO: implement catchup mechanism once storage is ready
        Ok(())
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
        max_log_size: 16 << 10, // 16 KiB
        max_events: 100,
        max_event_kind_size: 100,
        max_event_data_size: 16 << 10,               // 16 KiB
        max_storage_key_size: (1 << 20).try_into()?, // 1 MiB
        max_storage_value_size: (10 << 20).try_into()?, // 10 MiB
                                                     // can_write: writes, // todo!
    })
}
