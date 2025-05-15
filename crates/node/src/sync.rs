use std::collections::{hash_map, HashMap};
use std::pin::pin;
use std::time::Duration;

use calimero_context_primitives::client::ContextClient;
use calimero_crypto::{Nonce, SharedKey};
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::stream::{Message, Stream};
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::sync::{InitPayload, StreamMessage};
use calimero_primitives::context::ContextId;
use eyre::{bail, OptionExt};
use futures_util::stream::FuturesUnordered;
use futures_util::{FutureExt, SinkExt, StreamExt};
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;
use rand::{thread_rng, Rng};
use tokio::time::{self, timeout, Instant, MissedTickBehavior};
use tracing::{debug, error, warn};

mod blobs;
mod key;
mod state;

#[derive(Copy, Clone, Debug)]
pub struct SyncConfig {
    pub timeout: Duration,
    pub interval: Duration,
}

#[derive(Clone, Debug)]
pub struct SyncManager {
    sync_config: SyncConfig,

    node_client: NodeClient,
    context_client: ContextClient,
    network_client: NetworkClient,
}

#[derive(Debug)]
struct SyncState {
    last_sync: Option<Instant>,
}

#[derive(Default)]
struct Sequencer {
    current: usize,
}

impl Sequencer {
    fn current(&self) -> usize {
        self.current
    }

    fn test(&mut self, idx: usize) -> eyre::Result<()> {
        if self.current != idx {
            bail!(
                "out of sequence message: expected {}, got {}",
                self.current,
                idx
            );
        }

        self.current += 1;

        Ok(())
    }

    fn next(&mut self) -> usize {
        let current = self.current;
        self.current += 1;
        current
    }
}

async fn choose_stream<T>(stream: impl StreamExt<Item = T>, rng: &mut impl Rng) -> Option<T> {
    let mut stream = pin!(stream);

    let mut item = stream.next().await;

    let mut stream = stream.enumerate();

    while let Some((idx, this)) = stream.next().await {
        if rng.gen_range(0..idx + 1) == 0 {
            item = Some(this);
        }
    }

    item
}

impl SyncManager {
    pub fn new(
        sync_config: SyncConfig,
        node_client: NodeClient,
        context_client: ContextClient,
        network_client: NetworkClient,
    ) -> Self {
        Self {
            sync_config,
            node_client,
            context_client,
            network_client,
        }
    }

    pub async fn start(self) {
        let mut interval = time::interval(self.sync_config.interval);

        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let mut state = HashMap::<_, SyncState>::new();

        let mut futs = FuturesUnordered::new();

        let advance = async |futs: &mut FuturesUnordered<_>, state: &mut HashMap<_, SyncState>| {
            let (context_id, result) = futs.next().await?;

            let _ignored = state
                .entry(context_id)
                .and_modify(|state| state.last_sync = Some(Instant::now()));

            if let Err(_) = result {
                warn!(%context_id, "Timeout while performing sync");
            } else {
                debug!(%context_id, "Sync finished successfully");
            }

            Some(())
        };

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                Some(()) = async {
                    loop { advance(&mut futs, &mut state).await? }
                } => {},
            }

            let contexts = self.context_client.get_contexts(None);

            let mut contexts = pin!(contexts);

            while let Some(context_id) = contexts.next().await {
                let context_id = match context_id {
                    Ok(context_id) => context_id,
                    Err(err) => {
                        error!(%err, "Failed to get context id");
                        continue;
                    }
                };

                match state.entry(context_id) {
                    hash_map::Entry::Occupied(state) => {
                        let state = state.into_mut();

                        let Some(last_sync) = state.last_sync else {
                            debug!(
                                %context_id,
                                "Sync already in progress"
                            );

                            continue;
                        };

                        let long_ago = last_sync.elapsed();

                        if long_ago + Duration::from_secs(1) < self.sync_config.interval {
                            debug!(
                                %context_id,
                                long_ago=%long_ago.as_secs(),
                                "Skipping sync, last sync was too recent"
                            );

                            continue;
                        }

                        let _ignored = state.last_sync.take();
                    }
                    hash_map::Entry::Vacant(state) => {
                        debug!(
                            %context_id,
                            "Sync not started yet, starting now"
                        );

                        let _ignored = state.insert(SyncState { last_sync: None });
                    }
                };

                debug!(
                    %context_id,
                    "Performing interval triggered sync"
                );

                futs.push(
                    timeout(
                        self.sync_config.timeout,
                        self.perform_interval_sync(context_id),
                    )
                    .map(move |res| (context_id, res)),
                );

                if futs.len() == 30 {
                    let _ignored = advance(&mut futs, &mut state).await;
                }
            }
        }
    }

    async fn perform_interval_sync(&self, context_id: ContextId) {
        let peers = self
            .network_client
            .mesh_peers(TopicHash::from_raw(context_id))
            .await;

        for peer_id in peers {
            debug!(%context_id, %peer_id, "Attempting to perform interval triggered sync");

            let Err(err) = self.initiate_sync(context_id, peer_id).await else {
                debug!(%context_id, %peer_id, "Interval triggered sync successfully finished");
                break;
            };

            error!(%err, "Failed to perform interval sync, trying another peer");
        }
    }

    async fn send(
        &self,
        stream: &mut Stream,
        message: &StreamMessage<'_>,
        shared_key: Option<(SharedKey, Nonce)>,
    ) -> eyre::Result<()> {
        let encoded = borsh::to_vec(message)?;

        let message = match shared_key {
            Some((key, nonce)) => key
                .encrypt(encoded, nonce)
                .ok_or_eyre("encryption failed")?,
            None => encoded,
        };

        stream.send(Message::new(message)).await?;

        Ok(())
    }

    async fn recv(
        &self,
        stream: &mut Stream,
        duration: Duration,
        shared_key: Option<(SharedKey, Nonce)>,
    ) -> eyre::Result<Option<StreamMessage<'static>>> {
        let Some(message) = timeout(duration, stream.next()).await?.transpose()? else {
            return Ok(None);
        };

        let message = message.data.into_owned();

        let decrypted = match shared_key {
            Some((key, nonce)) => key
                .decrypt(message, nonce)
                .ok_or_eyre("decryption failed")?,
            None => message,
        };

        let decoded = borsh::from_slice::<StreamMessage<'static>>(&decrypted)?;

        Ok(Some(decoded))
    }

    pub(crate) async fn initiate_sync(
        &self,
        context_id: ContextId,
        chosen_peer: PeerId,
    ) -> eyre::Result<()> {
        let mut context = self
            .context_client
            .sync_context_config(context_id, None)
            .await?;

        let Some(application) = self.node_client.get_application(&context.application_id)? else {
            bail!("application not found: {}", context.application_id);
        };

        let identities = self.context_client.context_members(&context.id, Some(true));

        let Some((our_identity, _)) = choose_stream(identities, &mut thread_rng())
            .await
            .transpose()?
        else {
            bail!("no identities found for context: {}", context.id);
        };

        let mut stream = self.network_client.open_stream(chosen_peer).await?;

        self.initiate_key_share_process(&mut context, our_identity, &mut stream)
            .await?;

        if !self.node_client.has_blob(&application.blob)? {
            self.initiate_blob_share_process(
                &context,
                our_identity,
                application.blob,
                application.size,
                &mut stream,
            )
            .await?;
        }

        self.initiate_state_sync_process(&mut context, our_identity, &mut stream)
            .await
    }

    pub async fn handle_opened_stream(&self, mut stream: Box<Stream>) {
        loop {
            match self.internal_handle_opened_stream(&mut stream).await {
                Ok(None) => break,
                Ok(Some(())) => {}
                Err(err) => {
                    error!(%err, "Failed to handle stream message");

                    if let Err(err) = self
                        .send(&mut stream, &StreamMessage::OpaqueError, None)
                        .await
                    {
                        error!(%err, "Failed to send error message");
                    }
                }
            }
        }
    }

    async fn internal_handle_opened_stream(&self, stream: &mut Stream) -> eyre::Result<Option<()>> {
        let Some(message) = self.recv(stream, self.sync_config.timeout, None).await? else {
            return Ok(None);
        };

        let (context_id, their_identity, payload, nonce) = match message {
            StreamMessage::Init {
                context_id,
                party_id,
                payload,
                next_nonce,
                ..
            } => (context_id, party_id, payload, next_nonce),
            unexpected @ (StreamMessage::Message { .. } | StreamMessage::OpaqueError) => {
                bail!("expected initialization handshake, got {:?}", unexpected)
            }
        };

        let Some(mut context) = self.context_client.get_context(&context_id)? else {
            bail!("context not found: {}", context_id);
        };

        let mut updated = None;

        if !self
            .context_client
            .has_member(&context_id, &their_identity)?
        {
            updated = Some(
                self.context_client
                    .sync_context_config(context_id, None)
                    .await?,
            );

            if !self
                .context_client
                .has_member(&context_id, &their_identity)?
            {
                bail!(
                    "unknown context member {} in context {}",
                    their_identity,
                    context_id
                );
            }
        }

        let identities = self.context_client.context_members(&context.id, Some(true));

        let Some((our_identity, _)) = choose_stream(identities, &mut thread_rng())
            .await
            .transpose()?
        else {
            bail!("no identities found for context: {}", context.id);
        };

        match payload {
            InitPayload::KeyShare => {
                self.handle_key_share_request(&context, our_identity, their_identity, stream, nonce)
                    .await?
            }
            InitPayload::BlobShare { blob_id } => {
                self.handle_blob_share_request(
                    &context,
                    our_identity,
                    their_identity,
                    blob_id,
                    stream,
                )
                .await?
            }
            InitPayload::StateSync {
                root_hash: their_root_hash,
                application_id: their_application_id,
            } => {
                if updated.is_none() && context.application_id != their_application_id {
                    updated = Some(
                        self.context_client
                            .sync_context_config(context_id, None)
                            .await?,
                    );
                }

                if let Some(updated) = updated {
                    context = updated;
                }

                self.handle_state_sync_request(
                    &mut context,
                    our_identity,
                    their_identity,
                    their_root_hash,
                    their_application_id,
                    stream,
                    nonce,
                )
                .await?
            }
        };

        Ok(Some(()))
    }
}
