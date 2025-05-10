#![allow(clippy::print_stdout, reason = "Acceptable for CLI")]
#![allow(
    clippy::multiple_inherent_impl,
    reason = "TODO: Check if this is necessary"
)]

use core::future::{pending, Future};
use core::pin::Pin;
use core::str;
use std::time::Duration;

use borsh::{from_slice, to_vec};
use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager, FileSystem};
use calimero_context::config::ContextConfig;
use calimero_context::ContextManager;
use calimero_context_config::repr::ReprTransmute;
use calimero_context_config::ProposalAction;
use calimero_crypto::{Nonce, SharedKey, NONCE_LEN};
use calimero_network::client::NetworkClient;
use calimero_network::config::NetworkConfig;
use calimero_network::types::{NetworkEvent, PeerId};
use calimero_node_primitives::{CallError, ExecutionRequest};
use calimero_primitives::alias::Alias;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::events::{
    ContextEvent, ContextEventPayload, ExecutionEvent, ExecutionEventPayload, NodeEvent,
    StateMutationPayload,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_runtime::logic::{Outcome, VMContext, VMLimits};
use calimero_runtime::Constraint;
use calimero_server::config::ServerConfig;
use calimero_store::config::StoreConfig;
use calimero_store::key::ContextMeta as ContextMetaKey;
use calimero_store::Store;
use calimero_store_rocksdb::RocksDB;
use camino::Utf8PathBuf;
use eyre::{bail, eyre, OptionExt, Result as EyreResult};
use libp2p::gossipsub::{IdentTopic, Message, TopicHash};
use libp2p::identity::Keypair;
use memchr::memmem;
use rand::{thread_rng, Rng};
use tokio::io::{stdin, AsyncBufReadExt, BufReader};
use tokio::select;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{interval_at, Instant};
use tracing::{debug, error, info, warn};

pub mod interactive_cli;
pub mod runtime_compat;
pub mod sync;
pub mod types;

use runtime_compat::RuntimeCompatStore;
use sync::SyncConfig;
use types::BroadcastMessage;

type BoxedFuture<T> = Pin<Box<dyn Future<Output = T>>>;

#[derive(Debug)]
#[non_exhaustive]
pub struct NodeConfig {
    pub home: Utf8PathBuf,
    pub identity: Keypair,
    pub network: NetworkConfig,
    pub sync: SyncConfig,
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
        sync: SyncConfig,
        datastore: StoreConfig,
        blobstore: BlobStoreConfig,
        context: ContextConfig,
        server: ServerConfig,
    ) -> Self {
        Self {
            home,
            identity,
            network,
            sync,
            datastore,
            blobstore,
            context,
            server,
        }
    }
}

#[derive(Debug)]
pub struct Node {
    sync_config: SyncConfig,
    store: Store,
    ctx_manager: ContextManager,
    network_client: NetworkClient,
    node_events: broadcast::Sender<NodeEvent>,
    server_config: ServerConfig,
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

    #[expect(trivial_casts, reason = "Necessary here")]
    let mut server = Box::pin(calimero_server::start(
        config.server.clone(),
        server_sender,
        ctx_manager.clone(),
        node_events.clone(),
        store.clone(),
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
            .checked_add(Duration::from_millis(thread_rng().gen_range(1000..5000)))
            .ok_or_else(|| eyre!("Overflow when calculating initial catchup interval delay"))?,
        config.sync.interval,
    );

    let mut node = Node::new(
        config.sync,
        network_client,
        node_events,
        ctx_manager,
        store,
        config.server,
    );

    #[expect(clippy::redundant_pub_crate, reason = "Tokio code")]
    loop {
        select! {
            event = network_events.recv() => {
                let Some(event) = event else {
                    break;
                };
                node.handle_event(event).await;
            }
            line = stdin.next_line() => {
                if let Some(line) = line? {
                    if let Err(err) = interactive_cli::handle_line(&mut node, line).await {
                        error!("Failed handling user command: {:?}", err);
                    }
                }
            }
            result = &mut server => {
                result?;
                server = Box::pin(pending());
                continue;
            }
            Some(request) = server_receiver.recv() => node.handle_server_request(request).await,
            _ = catchup_interval_tick.tick() => node.perform_interval_sync().await,
        }
    }

    Ok(())
}

impl Node {
    #[must_use]
    pub const fn new(
        sync_config: SyncConfig,
        network_client: NetworkClient,
        node_events: broadcast::Sender<NodeEvent>,
        ctx_manager: ContextManager,
        store: Store,
        server_config: ServerConfig,
    ) -> Self {
        Self {
            sync_config,
            store,
            ctx_manager,
            network_client,
            node_events,
            server_config,
        }
    }

    pub async fn handle_event(&mut self, event: NetworkEvent) {
        match event {
            NetworkEvent::ListeningOn { address, .. } => {
                info!("Listening on: {}", address);
            }
            NetworkEvent::Subscribed {
                peer_id: their_peer_id,
                topic: topic_hash,
            } => {
                if let Err(err) = self.handle_subscribed(their_peer_id, &topic_hash) {
                    error!(?err, "Failed to handle subscribed event");
                }
            }
            NetworkEvent::Unsubscribed {
                peer_id: their_peer_id,
                topic: topic_hash,
            } => {
                if let Err(err) = self.handle_unsubscribed(their_peer_id, &topic_hash) {
                    error!(?err, "Failed to handle unsubscribed event");
                }
            }
            NetworkEvent::Message { message, .. } => {
                if let Err(err) = self.handle_message(message).await {
                    error!(?err, "Failed to handle message event");
                }
            }
            NetworkEvent::StreamOpened { peer_id, stream } => {
                debug!(%peer_id, "Stream opened!");

                self.handle_opened_stream(stream).await;

                debug!(%peer_id, "Stream closed!");
            }
            _ => error!("Unhandled event: {:?}", event),
        }
    }

    fn handle_subscribed(&self, their_peer_id: PeerId, topic_hash: &TopicHash) -> EyreResult<()> {
        let Ok(context_id) = topic_hash.as_str().parse() else {
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
        }

        info!(
            "Peer '{}' subscribed to context '{}'",
            their_peer_id, context_id
        );

        Ok(())
    }

    fn handle_unsubscribed(&self, their_peer_id: PeerId, topic_hash: &TopicHash) -> EyreResult<()> {
        let Ok(context_id) = topic_hash.as_str().parse() else {
            return Ok(());
        };

        let handle = self.store.handle();

        if !handle.has(&ContextMetaKey::new(context_id))? {
            debug!(
                %context_id,
                %their_peer_id,
                "Observed unsubscription from unknown context, ignoring.."
            );
            return Ok(());
        }

        info!(
            "Peer '{}' unsubscribed from context '{}'",
            their_peer_id, context_id
        );

        Ok(())
    }

    async fn handle_message(&mut self, message: Message) -> EyreResult<()> {
        let Some(source) = message.source else {
            warn!(?message, "Received message without source");
            return Ok(());
        };

        let message = from_slice::<BroadcastMessage<'static>>(&message.data)?;

        match message {
            BroadcastMessage::StateDelta {
                context_id,
                author_id,
                root_hash,
                artifact,
                nonce,
            } => {
                self.handle_state_delta(
                    source,
                    context_id,
                    author_id,
                    root_hash,
                    artifact.into_owned(),
                    nonce,
                )
                .await?;
            }
        }

        Ok(())
    }

    async fn handle_state_delta(
        &mut self,
        source: PeerId,
        context_id: ContextId,
        author_id: PublicKey,
        root_hash: Hash,
        artifact: Vec<u8>,
        nonce: [u8; NONCE_LEN],
    ) -> EyreResult<()> {
        let Some(mut context) = self.ctx_manager.get_context(&context_id)? else {
            bail!("context '{}' not found", context_id);
        };

        debug!(
            %context_id, %author_id,
            expected_root_hash = %root_hash,
            current_root_hash = %context.root_hash,
            "Received state delta"
        );

        if root_hash == context.root_hash {
            debug!(%context_id, "Received state delta with same root hash, ignoring..");
            return Ok(());
        }

        let Some(sender_key) = self.ctx_manager.get_sender_key(&context_id, &author_id)? else {
            debug!(%author_id, %context_id, "Missing sender key, initiating sync");

            return self.initiate_sync(context_id, source).await;
        };

        let shared_key = SharedKey::from_sk(&sender_key);

        let Some(artifact) = shared_key.decrypt(artifact, nonce) else {
            debug!(%author_id, %context_id, "State delta decryption failed, initiating sync");

            return self.initiate_sync(context_id, source).await;
        };

        let Some(outcome) = self
            .execute(&mut context, "__calimero_sync_next", artifact, author_id)
            .await?
        else {
            bail!("application not installed");
        };

        if let Some(derived_root_hash) = outcome.root_hash {
            if derived_root_hash != *root_hash {
                self.initiate_sync(context_id, source).await?;
            }
        }

        Ok(())
    }

    async fn send_state_delta(
        &self,
        context: &Context,
        outcome: &Outcome,
        executor_public_key: PublicKey,
    ) -> EyreResult<()> {
        debug!(
            %context.id,
            executor = %executor_public_key,
            %context.root_hash,
            "Sending state delta"
        );

        if self
            .network_client
            .mesh_peer_count(TopicHash::from_raw(context.id))
            .await
            != 0
        {
            let sender_key = self
                .ctx_manager
                .get_sender_key(&context.id, &executor_public_key)?
                .ok_or_eyre("expected own identity to have sender key")?;

            let shared_key = SharedKey::from_sk(&sender_key);
            let nonce = thread_rng().gen::<Nonce>();

            let artifact_encrypted = shared_key
                .encrypt(outcome.artifact.clone(), nonce)
                .ok_or_eyre("encryption failed")?;

            let message = to_vec(&BroadcastMessage::StateDelta {
                context_id: context.id,
                author_id: executor_public_key,
                root_hash: context.root_hash,
                artifact: artifact_encrypted.as_slice().into(),
                nonce,
            })?;

            let _ignored = self
                .network_client
                .publish(TopicHash::from_raw(context.id), message)
                .await?;
        }

        Ok(())
    }

    pub async fn handle_server_request(&mut self, request: ExecutionRequest) {
        let result = self
            .handle_call(
                request.context_id,
                &request.method,
                request.payload,
                request.executor_public_key,
                request.substitutes,
            )
            .await;

        if let Err(err) = request.outcome_sender.send(result) {
            error!(?err, "failed to respond to client request");
        }
    }

    async fn handle_call(
        &mut self,
        context_id: ContextId,
        method: &str,
        mut payload: Vec<u8>,
        executor_public_key: PublicKey,
        aliases: Vec<Alias<PublicKey>>,
    ) -> Result<Outcome, CallError> {
        let Ok(Some(mut context)) = self.ctx_manager.get_context(&context_id) else {
            return Err(CallError::ContextNotFound);
        };

        if method != "init" && &*context.root_hash == &[0; 32] {
            return Err(CallError::Uninitialized);
        }

        if !self
            .ctx_manager
            .context_has_owned_identity(context_id, executor_public_key)
            .unwrap_or_default()
        {
            return Err(CallError::Unauthorized {
                context_id,
                public_key: executor_public_key,
            });
        }

        if !aliases.is_empty() {
            payload = self
                .substitute_aliases_in_payload(context_id, payload, &aliases)
                .await?
        }

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

        if outcome.returns.is_err() {
            return Ok(outcome);
        }

        for (proposal_id, actions) in &outcome.proposals {
            let actions: Vec<ProposalAction> = from_slice(actions).map_err(|e| {
                error!(%e, "Failed to deserialize proposal actions.");
                CallError::InternalError
            })?;

            let proposal_id = proposal_id.rt().expect("infallible conversion");

            self.ctx_manager
                .propose(
                    context_id,
                    executor_public_key,
                    proposal_id,
                    actions.clone(),
                )
                .await
                .map_err(|e| {
                    error!(%e, "Failed to create proposal {:?}", proposal_id);
                    CallError::InternalError
                })?;
        }

        for proposal_id in &outcome.approvals {
            let proposal_id = proposal_id.rt().expect("infallible conversion");

            self.ctx_manager
                .approve(context_id, executor_public_key, proposal_id)
                .await
                .map_err(|e| {
                    error!(%e, "Failed to approve proposal {:?}", proposal_id);
                    CallError::InternalError
                })?;
        }

        if !outcome.artifact.is_empty() {
            if let Err(err) = self
                .send_state_delta(&context, &outcome, executor_public_key)
                .await
            {
                error!(%err, "Failed to send state delta.");
            }
        }

        Ok(outcome)
    }

    async fn substitute_aliases_in_payload(
        &self,
        context_id: ContextId,
        payload: Vec<u8>,
        aliases: &[Alias<PublicKey>],
    ) -> Result<Vec<u8>, CallError> {
        if aliases.is_empty() {
            return Ok(payload);
        }

        let mut result = Vec::with_capacity(payload.len());
        let mut remaining = &payload[..];

        for alias in aliases {
            let needle_str = format!("{{{alias}}}");
            let needle = needle_str.into_bytes();

            while let Some(pos) = memmem::find(remaining, &needle) {
                result.extend_from_slice(&remaining[..pos]);

                let public_key = self
                    .ctx_manager
                    .resolve_alias(*alias, Some(context_id))
                    .map_err(|_| CallError::InternalError)?
                    .ok_or_else(|| CallError::AliasResolutionFailed { alias: *alias })?;

                result.extend_from_slice(public_key.as_str().as_bytes());

                remaining = &remaining[pos + needle.len()..];
            }
        }

        result.extend_from_slice(remaining);

        Ok(result)
    }

    async fn execute(
        &self,
        context: &mut Context,
        method: &str,
        payload: Vec<u8>,
        executor_public_key: PublicKey,
    ) -> EyreResult<Option<Outcome>> {
        let Some(blob) = self
            .ctx_manager
            .load_application_blob(&context.application_id)
            .await?
        else {
            return Ok(None);
        };

        let mut store = self.store.clone();

        let mut storage = RuntimeCompatStore::new(&mut store, context.id);

        let outcome = calimero_runtime::run(
            &blob,
            &method,
            VMContext::new(payload, *context.id, *executor_public_key),
            &mut storage,
            &get_runtime_limits()?,
        )?;

        if outcome.returns.is_ok() {
            if let Some(root_hash) = outcome.root_hash {
                if outcome.artifact.is_empty() && method != "__calimero_sync_next" {
                    eyre::bail!("context state changed, but no actions were generated, discarding execution outcome to mitigate potential state inconsistency");
                }

                context.root_hash = root_hash.into();

                drop(self.node_events.send(NodeEvent::Context(ContextEvent::new(
                    context.id,
                    ContextEventPayload::StateMutation(StateMutationPayload::new(
                        context.root_hash,
                    )),
                ))));

                self.ctx_manager.save_context(context)?;
            }

            if !storage.is_empty() {
                storage.commit()?;
            }

            drop(
                self.node_events.send(NodeEvent::Context(ContextEvent::new(
                    context.id,
                    ContextEventPayload::ExecutionEvent(ExecutionEventPayload::new(
                        outcome
                            .events
                            .iter()
                            .map(|e| ExecutionEvent::new(e.kind.clone(), e.data.clone()))
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
    Ok(VMLimits {
        max_memory_pages: 1 << 10, // 1 KiB
        max_stack_size: 200 << 10, // 200 KiB
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
