use std::collections::{hash_map, HashMap};
use std::num::NonZeroUsize;
use std::pin::pin;

use calimero_context_primitives::client::ContextClient;
use calimero_crypto::{Nonce, SharedKey};
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::stream::{Message, Stream};
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::sync::{InitPayload, StreamMessage};
use calimero_primitives::context::ContextId;
use eyre::{bail, OptionExt, WrapErr};
use futures_util::stream::{self, FuturesUnordered};
use futures_util::{FutureExt, SinkExt, StreamExt, TryStreamExt};
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;
use rand::seq::SliceRandom;
use tokio::sync::mpsc;
use tokio::time::{self, timeout, timeout_at, Instant, MissedTickBehavior};
use tracing::{debug, error, info, warn};

use crate::utils::choose_stream;

mod blobs;
mod delta;
mod key;
mod state;

#[derive(Copy, Clone, Debug)]
pub struct SyncConfig {
    pub timeout: time::Duration,
    pub interval: time::Duration,
    pub frequency: time::Duration,
}

#[derive(Debug)]
pub(crate) struct SyncManager {
    sync_config: SyncConfig,

    node_client: NodeClient,
    context_client: ContextClient,
    network_client: NetworkClient,

    ctx_sync_rx: Option<mpsc::Receiver<(Option<ContextId>, Option<PeerId>)>>,
}

impl Clone for SyncManager {
    fn clone(&self) -> Self {
        Self {
            sync_config: self.sync_config.clone(),
            node_client: self.node_client.clone(),
            context_client: self.context_client.clone(),
            network_client: self.network_client.clone(),
            ctx_sync_rx: None,
        }
    }
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

impl SyncManager {
    pub fn new(
        sync_config: SyncConfig,
        node_client: NodeClient,
        context_client: ContextClient,
        network_client: NetworkClient,
        ctx_sync_rx: mpsc::Receiver<(Option<ContextId>, Option<PeerId>)>,
    ) -> Self {
        Self {
            sync_config,
            node_client,
            context_client,
            network_client,
            ctx_sync_rx: Some(ctx_sync_rx),
        }
    }

    pub async fn start(mut self) {
        let mut next_sync = time::interval(self.sync_config.frequency);

        next_sync.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let mut state = HashMap::<_, SyncState>::new();

        let mut futs = FuturesUnordered::new();

        let advance = async |futs: &mut FuturesUnordered<_>, state: &mut HashMap<_, SyncState>| {
            let (context_id, start, result) = futs.next().await?;

            let now = Instant::now();

            let _ignored = state
                .entry(context_id)
                .and_modify(|state| state.last_sync = Some(now));

            let took = Instant::saturating_duration_since(&now, start);

            if let Ok(_) = result {
                info!(%context_id, ?took, "Sync finished");
            } else {
                error!(%context_id, ?took, "Sync timed out");
            }

            Some(())
        };

        let mut requested_ctx = None;
        let mut requested_peer = None;

        let Some(mut ctx_sync_rx) = self.ctx_sync_rx.take() else {
            error!("SyncManager can only be run once");

            return;
        };

        loop {
            tokio::select! {
                _ = next_sync.tick() => {
                    debug!("Performing interval sync");
                }
                Some(()) = async {
                    loop { advance(&mut futs, &mut state).await? }
                } => {},
                Some((ctx, peer)) = ctx_sync_rx.recv() => {
                    info!(?ctx, ?peer, "Received sync request");

                    requested_ctx = ctx;
                    requested_peer = peer;
                }
            }

            let requested_ctx = requested_ctx.take();
            let requested_peer = requested_peer.take();

            let contexts = requested_ctx
                .is_none()
                .then(|| self.context_client.get_context_ids(None));

            let contexts = stream::iter(requested_ctx)
                .map(Ok)
                .chain(stream::iter(contexts).flatten());

            let mut contexts = pin!(contexts);

            while let Some(context_id) = contexts.next().await {
                let context_id = match context_id {
                    Ok(context_id) => context_id,
                    Err(err) => {
                        error!(%err, "Failed reading context id to sync");
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

                        let minimum = self.sync_config.interval;
                        let time_since = last_sync.elapsed();

                        if time_since < minimum {
                            if requested_ctx.is_none() {
                                debug!(%context_id, ?time_since, ?minimum, "Skipping sync, last one was too recent");

                                continue;
                            }

                            debug!(%context_id, ?time_since, ?minimum, "Force syncing despite recency, due to explicit request");
                        }

                        let _ignored = state.last_sync.take();
                    }
                    hash_map::Entry::Vacant(state) => {
                        info!(
                            %context_id,
                            "Syncing for the first time"
                        );

                        let _ignored = state.insert(SyncState { last_sync: None });
                    }
                };

                info!(%context_id, "Scheduled sync");

                let start = Instant::now();
                let Some(deadline) = start.checked_add(self.sync_config.timeout) else {
                    error!(
                        ?start,
                        timeout=?self.sync_config.timeout,
                        "Unable to determine when to timeout sync procedure"
                    );

                    // if we can't determine the sync deadline, this is a hard error
                    // we intentionally want to exit the sync loop
                    return;
                };

                let fut = timeout_at(
                    deadline,
                    self.perform_interval_sync(context_id, requested_peer),
                )
                .map(move |res| (context_id, start, res));

                futs.push(fut);

                if futs.len() == 30 {
                    let _ignored = advance(&mut futs, &mut state).await;
                }
            }
        }
    }

    async fn perform_interval_sync(&self, context_id: ContextId, peer_id: Option<PeerId>) {
        if let Some(peer_id) = peer_id {
            let _ignored = self.initiate_sync(context_id, peer_id).await;

            return;
        }

        let peers = self
            .network_client
            .mesh_peers(TopicHash::from_raw(context_id))
            .await;

        if peers.is_empty() {
            debug!(%context_id, "No peers to sync with");
        }

        for peer_id in peers.choose_multiple(&mut rand::thread_rng(), peers.len()) {
            if self.initiate_sync(context_id, *peer_id).await {
                break;
            }
        }
    }

    async fn initiate_sync(&self, context_id: ContextId, peer_id: PeerId) -> bool {
        let start = Instant::now();

        info!(%context_id, %peer_id, "Attempting to sync with peer");

        let res = self.initiate_sync_inner(context_id, peer_id).await;

        let took = start.elapsed();

        let Err(err) = res else {
            info!(%context_id, %peer_id, ?took, "Sync with peer completed successfully");

            return true;
        };

        error!(%context_id, %peer_id, ?took, %err, "Failed to sync with peer");

        false
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
        shared_key: Option<(SharedKey, Nonce)>,
    ) -> eyre::Result<Option<StreamMessage<'static>>> {
        let budget = self.sync_config.timeout / 3;

        let message = timeout(budget, stream.try_next())
            .await
            .wrap_err("timeout receiving message from ")?
            .wrap_err("error receiving message from peer")?;

        let Some(message) = message else {
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

    async fn initiate_sync_inner(
        &self,
        context_id: ContextId,
        chosen_peer: PeerId,
    ) -> eyre::Result<()> {
        const MAX_DELTA_GAP_FOR_DELTA_SYNC: usize = 128;

        let mut context = self
            .context_client
            .sync_context_config(context_id, None)
            .await?;

        let Some(application) = self.node_client.get_application(&context.application_id)? else {
            bail!("application not found: {}", context.application_id);
        };

        let identities = self
            .context_client
            .get_context_members(&context.id, Some(true));

        let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
            .await
            .transpose()?
        else {
            bail!("no owned identities found for context: {}", context.id);
        };

        let mut stream = self.network_client.open_stream(chosen_peer).await?;

        self.initiate_key_share_process(&mut context, our_identity, &mut stream)
            .await?;

        if !self.node_client.has_blob(&application.blob.bytecode)? {
            self.initiate_blob_share_process(
                &context,
                our_identity,
                application.blob.bytecode,
                application.size,
                &mut stream,
            )
            .await?;
        }

        // Determine whether to use delta sync or state sync
        // Check if we're too far behind for delta sync to be efficient
        let should_use_state_sync = self
            .should_use_state_sync(&context, MAX_DELTA_GAP_FOR_DELTA_SYNC)
            .await?;

        if should_use_state_sync {
            info!(
                context_id=%context.id,
                "Delta gap exceeds {}, using state sync instead of delta sync",
                MAX_DELTA_GAP_FOR_DELTA_SYNC
            );
            self.initiate_state_sync_process(&mut context, our_identity, &mut stream)
                .await
        } else {
            // Try delta sync first, fall back to state sync on failure
            match self
                .initiate_delta_sync_process(&mut context, our_identity, &mut stream)
                .await
            {
                Ok(()) => Ok(()),
                Err(e) => {
                    warn!(
                        context_id=%context.id,
                        error=%e,
                        "Delta sync failed, falling back to state sync"
                    );
                    self.initiate_state_sync_process(&mut context, our_identity, &mut stream)
                        .await
                }
            }
        }
    }

    async fn should_use_state_sync(
        &self,
        context: &calimero_primitives::context::Context,
        max_gap: usize,
    ) -> eyre::Result<bool> {
        // Check all members to see if any have a gap larger than max_gap
        let members = self.context_client.get_context_members(&context.id, None);
        let members = pin!(members).try_collect::<Vec<_>>().await?;

        for (member, _) in members {
            if let Some(height) = self.context_client.get_delta_height(&context.id, &member)? {
                // If we have a height tracked but no recent deltas, we might be missing too many
                // This is a heuristic - if height is suspiciously high, we're probably missing deltas
                if height.get() > max_gap {
                    // Check if we actually have deltas from (height - max_gap) onwards
                    // If we're missing them, we should use state sync
                    let check_height = NonZeroUsize::new(height.get().saturating_sub(max_gap))
                        .unwrap_or(NonZeroUsize::MIN);

                    let deltas = self.context_client.get_state_deltas(
                        &context.id,
                        Some(&member),
                        check_height,
                    );

                    let count = pin!(deltas).count().await;

                    // If we have fewer deltas than expected, use state sync
                    if count < max_gap.min(height.get()) {
                        debug!(
                            context_id=%context.id,
                            member=%member,
                            expected_deltas=%max_gap.min(height.get()),
                            found_deltas=%count,
                            "Insufficient deltas available, will use state sync"
                        );
                        return Ok(true);
                    }
                }
            }
        }

        Ok(false)
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
        let Some(message) = self.recv(stream, None).await? else {
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

        // todo! prevent initiating sync once we are already syncing

        let identities = self
            .context_client
            .get_context_members(&context.id, Some(true));

        let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
            .await
            .transpose()?
        else {
            bail!("no owned identities found for context: {}", context.id);
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
            InitPayload::DeltaSync {
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

                self.handle_delta_sync_request(
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
