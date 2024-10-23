#![allow(clippy::print_stdout, reason = "Acceptable for CLI")]
#![allow(
    clippy::multiple_inherent_impl,
    reason = "TODO: Check if this is necessary"
)]

use core::future::{pending, Future};
use core::pin::Pin;

use borsh::to_vec;
use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager, FileSystem};
use calimero_context::config::ContextConfig;
use calimero_context::ContextManager;
use calimero_network::client::NetworkClient;
use calimero_network::config::NetworkConfig;
use calimero_network::types::{NetworkEvent, PeerId};
use calimero_node_primitives::{CallError, ExecutionRequest};
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::events::{
    ApplicationEvent, ApplicationEventPayload, ExecutedTransactionPayload, NodeEvent, OutcomeEvent,
    OutcomeEventPayload, PeerJoinedPayload,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_runtime::logic::{Outcome, VMContext, VMLimits};
use calimero_runtime::Constraint;
use calimero_server::config::ServerConfig;
use calimero_storage::address::Id;
use calimero_storage::integration::Comparison;
use calimero_storage::interface::Action;
use calimero_store::config::StoreConfig;
use calimero_store::db::RocksDB;
use calimero_store::key::ContextMeta as ContextMetaKey;
use calimero_store::Store;
use camino::Utf8PathBuf;
use eyre::{bail, eyre, Result as EyreResult};
use libp2p::gossipsub::{IdentTopic, Message, TopicHash};
use libp2p::identity::Keypair;
use owo_colors::OwoColorize;
use serde_json::{from_slice as from_json_slice, to_vec as to_json_vec};
use tokio::io::{stdin, AsyncBufReadExt, BufReader};
use tokio::select;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{interval_at, Instant};
use tracing::{debug, error, info, warn};

use crate::runtime_compat::RuntimeCompatStore;
use crate::types::{ActionMessage, PeerAction, SyncMessage};

pub mod catchup;
pub mod interactive_cli;
pub mod runtime_compat;
pub mod types;

type BoxedFuture<T> = Pin<Box<dyn Future<Output = T>>>;

#[derive(Debug)]
#[non_exhaustive]
pub struct NodeConfig {
    pub home: Utf8PathBuf,
    pub identity: Keypair,
    pub network: NetworkConfig,
    pub datastore: StoreConfig,
    pub blobstore: BlobStoreConfig,
    pub context: ContextConfig,
    pub server: ServerConfig,
}

impl NodeConfig {
    #[must_use]
    pub const fn new(
        home: Utf8PathBuf,
        identity: Keypair,
        network: NetworkConfig,
        datastore: StoreConfig,
        blobstore: BlobStoreConfig,
        context: ContextConfig,
        server: ServerConfig,
    ) -> Self {
        Self {
            home,
            identity,
            network,
            datastore,
            blobstore,
            context,
            server,
        }
    }
}

#[derive(Debug)]
pub struct Node {
    store: Store,
    ctx_manager: ContextManager,
    network_client: NetworkClient,
    node_events: broadcast::Sender<NodeEvent>,
}

pub async fn start(config: NodeConfig) -> EyreResult<()> {
    let peer_id = config.identity.public().to_peer_id();

    info!("Peer ID: {}", peer_id);

    let (node_events, _) = broadcast::channel(32);

    let (network_client, mut network_events) = calimero_network::run(&config.network).await?;

    let store = Store::open::<RocksDB>(&config.datastore)?;

    let blob_manager = BlobManager::new(store.clone(), FileSystem::new(&config.blobstore).await?);

    let (server_sender, mut server_receiver) = mpsc::channel(32);

    let ctx_manager = ContextManager::start(
        &config.context,
        store.clone(),
        blob_manager,
        server_sender.clone(),
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

    #[expect(trivial_casts, reason = "Necessary here")]
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

    #[expect(clippy::redundant_pub_crate, reason = "Tokio code")]
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
                    if let Err(err) = interactive_cli::handle_line(&mut node, line).await {
                        error!("Failed to handle line: {:?}", err);
                    }
                }
            }
            result = &mut server => {
                result?;
                server = Box::pin(pending());
                continue;
            }
            Some(request) = server_receiver.recv() => node.handle_call(request).await,
            _ = catchup_interval_tick.tick() => node.perform_interval_catchup().await,
        }
    }

    Ok(())
}

impl Node {
    #[must_use]
    pub const fn new(
        _config: &NodeConfig,
        network_client: NetworkClient,
        node_events: broadcast::Sender<NodeEvent>,
        ctx_manager: ContextManager,
        store: Store,
    ) -> Self {
        Self {
            store,
            ctx_manager,
            network_client,
            node_events,
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
            PeerAction::ActionList(action_list) => {
                debug!(?action_list, %source, "Received action list");

                for action in action_list.actions {
                    debug!(?action, %source, "Received action");
                    let Some(context) = self.ctx_manager.get_context(&action_list.context_id)?
                    else {
                        bail!("Context '{}' not found", action_list.context_id);
                    };
                    match action {
                        Action::Compare { id } => {
                            self.send_comparison_message(&context, id, action_list.public_key)
                                .await
                        }
                        Action::Add { .. } | Action::Delete { .. } | Action::Update { .. } => {
                            self.apply_action(&context, &action, action_list.public_key)
                                .await
                        }
                    }?;
                }
                Ok(())
            }
            PeerAction::Sync(sync) => {
                debug!(?sync, %source, "Received sync request");

                let Some(context) = self.ctx_manager.get_context(&sync.context_id)? else {
                    bail!("Context '{}' not found", sync.context_id);
                };
                let outcome = self
                    .compare_trees(&context, &sync.comparison, sync.public_key)
                    .await?;

                match outcome.returns {
                    Ok(Some(actions_data)) => {
                        let (local_actions, remote_actions): (Vec<Action>, Vec<Action>) =
                            from_json_slice(&actions_data)?;

                        // Apply local actions
                        for action in local_actions {
                            match action {
                                Action::Compare { id } => {
                                    self.send_comparison_message(&context, id, sync.public_key)
                                        .await
                                }
                                Action::Add { .. }
                                | Action::Delete { .. }
                                | Action::Update { .. } => {
                                    self.apply_action(&context, &action, sync.public_key).await
                                }
                            }?;
                        }

                        if !remote_actions.is_empty() {
                            // Send remote actions back to the peer
                            // TODO: This just sends one at present - needs to send a batch
                            let new_message = ActionMessage {
                                actions: remote_actions,
                                context_id: sync.context_id,
                                public_key: sync.public_key,
                                root_hash: context.root_hash,
                            };
                            self.push_action(sync.context_id, PeerAction::ActionList(new_message))
                                .await?;
                        }
                    }
                    Ok(None) => {
                        // No actions needed
                    }
                    Err(err) => {
                        error!("Error during comparison: {err:?}");
                        // TODO: Handle the error appropriately
                    }
                }
                Ok(())
            }
        }
    }

    async fn send_comparison_message(
        &mut self,
        context: &Context,
        id: Id,
        public_key: PublicKey,
    ) -> EyreResult<()> {
        let compare_outcome = self
            .generate_comparison_data(context, id, public_key)
            .await?;
        match compare_outcome.returns {
            Ok(Some(comparison_data)) => {
                // Generate a new Comparison for this entity and send it to the peer
                let new_sync = SyncMessage {
                    comparison: from_json_slice(&comparison_data)?,
                    context_id: context.id,
                    public_key,
                    root_hash: context.root_hash,
                };
                self.push_action(context.id, PeerAction::Sync(new_sync))
                    .await?;
                Ok(())
            }
            Ok(None) => Err(eyre!("No comparison data generated")),
            Err(err) => Err(eyre!(err)),
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

    pub async fn handle_call(&mut self, request: ExecutionRequest) {
        let Ok(Some(context)) = self.ctx_manager.get_context(&request.context_id) else {
            drop(request.outcome_sender.send(Err(CallError::ContextNotFound {
                context_id: request.context_id,
            })));
            return;
        };

        let task = self.call_query(
            &context,
            request.method,
            request.payload,
            request.executor_public_key,
        );

        drop(request.outcome_sender.send(task.await.map_err(|err| {
            error!(%err, "failed to execute local query");

            CallError::InternalError
        })));
    }

    async fn call_query(
        &mut self,
        context: &Context,
        method: String,
        payload: Vec<u8>,
        executor_public_key: PublicKey,
    ) -> Result<Outcome, CallError> {
        let outcome_option = self
            .checked_execute(
                context,
                Some(context.root_hash),
                method,
                payload,
                executor_public_key,
            )
            .await
            .map_err(|e| {
                error!(%e, "Failed to execute query call.");
                CallError::InternalError
            })?;

        let Some(outcome) = outcome_option else {
            return Err(CallError::ApplicationNotInstalled {
                application_id: context.application_id,
            });
        };
        if self
            .network_client
            .mesh_peer_count(TopicHash::from_raw(context.id))
            .await
            != 0
        {
            let actions = outcome
                .actions
                .iter()
                .map(|a| borsh::from_slice(a))
                .collect::<Result<Vec<Action>, _>>()
                .map_err(|err| {
                    error!(%err, "Failed to deserialize actions.");
                    CallError::InternalError
                })?;
            self.push_action(
                context.id,
                PeerAction::ActionList(ActionMessage {
                    actions,
                    context_id: context.id,
                    public_key: executor_public_key,
                    root_hash: context.root_hash,
                }),
            )
            .await
            .map_err(|err| {
                error!(%err, "Failed to push action over the network.");
                CallError::InternalError
            })?;
        }

        Ok(outcome)
    }

    async fn apply_action(
        &mut self,
        context: &Context,
        action: &Action,
        public_key: PublicKey,
    ) -> EyreResult<()> {
        let outcome = self
            .checked_execute(
                context,
                None,
                "apply_action".to_owned(),
                to_vec(action)?,
                public_key,
            )
            .await
            .and_then(|outcome| outcome.ok_or_else(|| eyre!("Application not installed")))?;
        drop(outcome.returns?);
        Ok(())
    }

    async fn compare_trees(
        &mut self,
        context: &Context,
        comparison: &Comparison,
        public_key: PublicKey,
    ) -> EyreResult<Outcome> {
        self.checked_execute(
            context,
            None,
            "compare_trees".to_owned(),
            to_vec(comparison)?,
            public_key,
        )
        .await
        .and_then(|outcome| outcome.ok_or_else(|| eyre!("Application not installed")))
    }

    async fn generate_comparison_data(
        &mut self,
        context: &Context,
        id: Id,
        public_key: PublicKey,
    ) -> EyreResult<Outcome> {
        self.checked_execute(
            context,
            None,
            "generate_comparison_data".to_owned(),
            to_vec(&id)?,
            public_key,
        )
        .await
        .and_then(|outcome| outcome.ok_or_else(|| eyre!("Application not installed")))
    }

    async fn checked_execute(
        &mut self,
        context: &Context,
        hash: Option<Hash>,
        method: String,
        payload: Vec<u8>,
        executor_public_key: PublicKey,
    ) -> EyreResult<Option<Outcome>> {
        if !self
            .ctx_manager
            .is_application_installed(&context.application_id)
            .unwrap_or_default()
        {
            return Ok(None);
        }

        self.execute(context, hash, method, payload, executor_public_key)
            .await
            .map(Some)
    }

    async fn execute(
        &mut self,
        context: &Context,
        hash: Option<Hash>,
        method: String,
        payload: Vec<u8>,
        executor_public_key: PublicKey,
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
            VMContext::new(payload, context.id.into(), *executor_public_key),
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
