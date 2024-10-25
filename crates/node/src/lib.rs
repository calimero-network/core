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
use calimero_crypto::SharedKey;
use calimero_network::client::NetworkClient;
use calimero_network::config::NetworkConfig;
use calimero_network::types::{NetworkEvent, PeerId};
use calimero_node_primitives::{CallError, ExecutionRequest};
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::events::{
    ApplicationEvent, ApplicationEventPayload, NodeEvent, OutcomeEvent, OutcomeEventPayload,
    PeerJoinedPayload, StateMutationPayload,
};
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
use ring::aead;
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

// TODO: delete once miraclx lands catchup logic
pub fn get_shared_key() -> Result<SharedKey, ed25519_dalek::SignatureError> {
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&[
        0x1b, 0x2e, 0x3d, 0x4c, 0x5a, 0x69, 0x78, 0x87, 0x96, 0xa5, 0xb4, 0xc3, 0xd2, 0xe1, 0xf0,
        0x0f, 0x1e, 0x2d, 0x3c, 0x4b, 0x0f, 0x69, 0x78, 0x87, 0x96, 0xa5, 0xb4, 0xc3, 0xd2, 0xe1,
        0xf0, 0x00,
    ]);

    let verifying_key = ed25519_dalek::SigningKey::from_bytes(&[
        0x1b, 0x5e, 0x3d, 0x4c, 0x5a, 0x69, 0x78, 0x87, 0x96, 0xa5, 0xb4, 0xc3, 0xd2, 0xe1, 0xf0,
        0x3f, 0x1e, 0x0d, 0x3c, 0x4b, 0x5a, 0x69, 0x78, 0x87, 0x96, 0xa5, 0xb4, 0xc3, 0xd2, 0x01,
        0xf0, 0x00,
    ]);

    Ok(SharedKey::new(&signing_key, &verifying_key.verifying_key()))
}

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
            Some(request) = server_receiver.recv() => node.handle_server_request(request).await,
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

        // TODO: remove once miraclx lands catchup logic
        let decryption_key = get_shared_key().map_err(|err| eyre!(err))?;
        let action = from_json_slice::<PeerAction>(
            &decryption_key
                .decrypt(message.data, [0u8; aead::NONCE_LEN])
                .ok_or_else(|| eyre!("Failed to decrypt message"))?,
        )?;

        match action {
            PeerAction::ActionList(action_list) => {
                debug!(?action_list, %source, "Received action list");

                for action in action_list.actions {
                    debug!(?action, %source, "Received action");
                    let Some(mut context) =
                        self.ctx_manager.get_context(&action_list.context_id)?
                    else {
                        bail!("Context '{}' not found", action_list.context_id);
                    };
                    match action {
                        Action::Compare { id } => {
                            self.send_comparison_message(&mut context, id, action_list.public_key)
                                .await
                        }
                        Action::Add { .. } | Action::Delete { .. } | Action::Update { .. } => {
                            self.apply_action(&mut context, &action, action_list.public_key)
                                .await
                        }
                    }?;
                }
                Ok(())
            }
            PeerAction::Sync(sync) => {
                debug!(?sync, %source, "Received sync request");

                let Some(mut context) = self.ctx_manager.get_context(&sync.context_id)? else {
                    bail!("Context '{}' not found", sync.context_id);
                };
                let outcome = self
                    .compare_trees(&mut context, &sync.comparison, sync.public_key)
                    .await?;

                match outcome.returns {
                    Ok(Some(actions_data)) => {
                        let (local_actions, remote_actions): (Vec<Action>, Vec<Action>) =
                            from_json_slice(&actions_data)?;

                        // Apply local actions
                        for action in local_actions {
                            match action {
                                Action::Compare { id } => {
                                    self.send_comparison_message(&mut context, id, sync.public_key)
                                        .await
                                }
                                Action::Add { .. }
                                | Action::Delete { .. }
                                | Action::Update { .. } => {
                                    self.apply_action(&mut context, &action, sync.public_key)
                                        .await
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
            PeerAction::RequestSenderKey(request_sender_key_message) => {
                debug!(?request_sender_key_message, %source, "Received request sender key message");
                Ok(())
            }
        }
    }

    async fn send_comparison_message(
        &mut self,
        context: &mut Context,
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
        // TODO:: remove once miraclx lands catchup logic
        let encryption_key = get_shared_key().map_err(|err| eyre!(err))?;
        let data = encryption_key
            .encrypt(to_json_vec(&action)?, [0; aead::NONCE_LEN])
            .unwrap();
        drop(
            self.network_client
                .publish(TopicHash::from_raw(context_id), data)
                .await?,
        );

        Ok(())
    }

    pub async fn handle_server_request(&mut self, request: ExecutionRequest) {
        let task = self.handle_call(
            request.context_id,
            request.method,
            request.payload,
            request.executor_public_key,
        );

        drop(request.outcome_sender.send(task.await.map_err(|err| {
            error!(%err, "failed to execute local query");

            CallError::InternalError
        })));
    }

    async fn handle_call(
        &mut self,
        context_id: ContextId,
        method: String,
        payload: Vec<u8>,
        executor_public_key: PublicKey,
    ) -> Result<Outcome, CallError> {
        let Ok(Some(mut context)) = self.ctx_manager.get_context(&context_id) else {
            return Err(CallError::ContextNotFound { context_id });
        };

        let outcome_option = self
            .execute(&mut context, method, payload, executor_public_key)
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
        context: &mut Context,
        action: &Action,
        public_key: PublicKey,
    ) -> EyreResult<()> {
        let outcome = self
            .execute(
                context,
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
        context: &mut Context,
        comparison: &Comparison,
        public_key: PublicKey,
    ) -> EyreResult<Outcome> {
        self.execute(
            context,
            "compare_trees".to_owned(),
            to_vec(comparison)?,
            public_key,
        )
        .await
        .and_then(|outcome| outcome.ok_or_else(|| eyre!("Application not installed")))
    }

    async fn generate_comparison_data(
        &mut self,
        context: &mut Context,
        id: Id,
        public_key: PublicKey,
    ) -> EyreResult<Outcome> {
        self.execute(
            context,
            "generate_comparison_data".to_owned(),
            to_vec(&id)?,
            public_key,
        )
        .await
        .and_then(|outcome| outcome.ok_or_else(|| eyre!("Application not installed")))
    }

    async fn execute(
        &mut self,
        context: &mut Context,
        method: String,
        payload: Vec<u8>,
        executor_public_key: PublicKey,
    ) -> EyreResult<Option<Outcome>> {
        let mut storage = RuntimeCompatStore::new(&mut self.store, context.id);

        let Some(blob) = self
            .ctx_manager
            .load_application_blob(&context.application_id)
            .await?
        else {
            return Ok(None);
        };

        let outcome = calimero_runtime::run(
            &blob,
            &method,
            VMContext::new(payload, context.id.into(), *executor_public_key),
            &mut storage,
            &get_runtime_limits()?,
        )?;

        if outcome.returns.is_ok() {
            if let Some(root_hash) = outcome.root_hash {
                if outcome.actions.is_empty() {
                    eyre::bail!("Context state changed, but no actions were generated, discarding execution outcome to mitigate potential state inconsistency");
                }

                context.root_hash = root_hash.into();

                drop(
                    self.node_events
                        .send(NodeEvent::Application(ApplicationEvent::new(
                            context.id,
                            ApplicationEventPayload::StateMutation(StateMutationPayload::new(
                                context.root_hash,
                            )),
                        ))),
                );

                self.ctx_manager.save_context(context)?;
            }

            if !storage.is_empty() {
                storage.commit()?;
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
        }

        Ok(Some(outcome))
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
