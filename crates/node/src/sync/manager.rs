//! Sync manager and orchestration.
//!
//! **Purpose**: Coordinates periodic syncs, selects peers, and delegates to protocols.
//! **Strategy**: Try delta sync first, fallback to state sync on failure.

use std::collections::{hash_map, HashMap};
use std::pin::pin;

use calimero_context_primitives::client::ContextClient;
use calimero_crypto::{Nonce, SharedKey};
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::sync::{InitPayload, StreamMessage};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use eyre::bail;
use futures_util::stream::{self, FuturesUnordered};
use futures_util::{FutureExt, StreamExt};
use libp2p::PeerId;
use tokio::sync::mpsc;
use tokio::time::{self, timeout_at, Instant, MissedTickBehavior};
use tracing::{debug, error, info, warn};

use crate::utils::choose_stream;

use super::config::SyncConfig;
use super::direct::dag_bootstrapper::DagBootstrapper;
use super::direct::peer_selector::PeerSelector;
use super::direct::request_queue::RequestQueue;
use super::direct::stream_responder::StreamResponder;
use super::tracking::{SyncProtocol, SyncState};

/// Network synchronization manager.
///
/// Orchestrates sync protocols: full resync, delta sync, state sync.
#[derive(Debug)]
pub struct SyncManager {
    pub(crate) sync_config: SyncConfig,

    pub(super) node_client: NodeClient,
    pub(super) context_client: ContextClient,
    pub(crate) network_client: NetworkClient,
    pub(super) node_state: crate::NodeState,

    request_queue: Option<RequestQueue>,
    peer_selector: PeerSelector,
    dag_bootstrapper: DagBootstrapper,
    stream_responder: StreamResponder,
}

impl Clone for SyncManager {
    fn clone(&self) -> Self {
        Self {
            sync_config: self.sync_config.clone(),
            node_client: self.node_client.clone(),
            context_client: self.context_client.clone(),
            network_client: self.network_client.clone(),
            node_state: self.node_state.clone(),
            request_queue: None,
            peer_selector: PeerSelector::new(
                self.sync_config.clone(),
                self.network_client.clone(),
                self.context_client.clone(),
            ),
            dag_bootstrapper: DagBootstrapper::new(
                self.sync_config.clone(),
                self.context_client.clone(),
                self.network_client.clone(),
                self.node_state.clone(),
            ),
            stream_responder: StreamResponder::new(
                self.sync_config.clone(),
                self.node_client.clone(),
                self.context_client.clone(),
                self.node_state.clone(),
            ),
        }
    }
}

impl SyncManager {
    pub(crate) fn new(
        sync_config: SyncConfig,
        node_client: NodeClient,
        context_client: ContextClient,
        network_client: NetworkClient,
        node_state: crate::NodeState,
        ctx_sync_rx: mpsc::Receiver<(Option<ContextId>, Option<PeerId>)>,
    ) -> Self {
        let peer_selector = PeerSelector::new(
            sync_config.clone(),
            network_client.clone(),
            context_client.clone(),
        );

        let dag_bootstrapper = DagBootstrapper::new(
            sync_config.clone(),
            context_client.clone(),
            network_client.clone(),
            node_state.clone(),
        );

        let stream_responder = StreamResponder::new(
            sync_config.clone(),
            node_client.clone(),
            context_client.clone(),
            node_state.clone(),
        );

        let request_queue = RequestQueue::new(ctx_sync_rx);

        Self {
            sync_config,
            node_client,
            context_client,
            network_client,
            node_state,
            request_queue: Some(request_queue),
            peer_selector,
            dag_bootstrapper,
            stream_responder,
        }
    }

    pub async fn start(mut self) {
        let mut next_sync = time::interval(self.sync_config.frequency);

        next_sync.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let mut state = HashMap::<_, SyncState>::new();

        let mut futs = FuturesUnordered::new();

        let advance = async |futs: &mut FuturesUnordered<_>, state: &mut HashMap<_, SyncState>| {
            let (context_id, peer_id, start, result): (
                ContextId,
                PeerId,
                Instant,
                Result<Result<SyncProtocol, eyre::Error>, time::error::Elapsed>,
            ) = futs.next().await?;

            let now = Instant::now();
            let took = Instant::saturating_duration_since(&now, start);

            let _ignored = state.entry(context_id).and_modify(|state| match result {
                Ok(Ok(protocol)) => {
                    state.on_success(peer_id, protocol);
                    info!(
                        %context_id,
                        ?took,
                        ?protocol,
                        success_count = state.success_count,
                        "Sync finished successfully"
                    );
                }
                Ok(Err(ref err)) => {
                    state.on_failure(err.to_string());
                    warn!(
                        %context_id,
                        ?took,
                        error = %err,
                        failure_count = state.failure_count(),
                        backoff_secs = state.backoff_delay().as_secs(),
                        "Sync failed, applying exponential backoff"
                    );
                }
                Err(ref timeout_err) => {
                    state.on_failure(timeout_err.to_string());
                    warn!(
                        %context_id,
                        ?took,
                        failure_count = state.failure_count(),
                        backoff_secs = state.backoff_delay().as_secs(),
                        "Sync timed out, applying exponential backoff"
                    );
                }
            });

            Some(())
        };

        let mut requested_ctx = None;
        let mut requested_peer = None;

        let Some(mut request_queue) = self.request_queue.take() else {
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
                Some(event) = request_queue.next() => {
                    info!(ctx=?event.original_ctx, peer=?event.original_peer, "Received sync request");

                    if event.drained_count > 0 {
                        info!(drained_count = event.drained_count, "Drained additional sync requests from queue, will sync all contexts");
                    }

                    requested_ctx = event.requested_ctx;
                    requested_peer = event.requested_peer;
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

                        let Some(last_sync) = state.last_sync() else {
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

                        let _ignored = state.take_last_sync();
                    }
                    hash_map::Entry::Vacant(state) => {
                        info!(
                            %context_id,
                            "Syncing for the first time"
                        );

                        let mut new_state = SyncState::new();
                        new_state.start();
                        let _ignored = state.insert(new_state);
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
                .map(move |res| {
                    // Extract peer_id from result or use placeholder
                    let peer_id = res
                        .as_ref()
                        .ok()
                        .and_then(|r| r.as_ref().ok())
                        .map(|(p, _)| *p)
                        .unwrap_or(PeerId::random());
                    (
                        context_id,
                        peer_id,
                        start,
                        res.map(|r| r.map(|(_, proto)| proto)),
                    )
                });

                futs.push(fut);

                if futs.len() >= self.sync_config.max_concurrent {
                    let _ignored = advance(&mut futs, &mut state).await;
                }
            }
        }
    }

    async fn perform_interval_sync(
        &self,
        context_id: ContextId,
        peer_id: Option<PeerId>,
    ) -> eyre::Result<(PeerId, SyncProtocol)> {
        let candidate_peers = self
            .peer_selector
            .candidate_peers(context_id, peer_id)
            .await?;

        for peer_id in candidate_peers {
            if let Ok(result) = self.initiate_sync(context_id, peer_id).await {
                return Ok(result);
            }
        }

        bail!("Failed to sync with any peer for context {}", context_id)
    }

    async fn initiate_sync(
        &self,
        context_id: ContextId,
        peer_id: PeerId,
    ) -> eyre::Result<(PeerId, SyncProtocol)> {
        let start = Instant::now();

        info!(%context_id, %peer_id, "Attempting to sync with peer");

        let protocol = self.initiate_sync_inner(context_id, peer_id).await?;

        let took = start.elapsed();

        info!(%context_id, %peer_id, ?took, ?protocol, "Sync with peer completed successfully");

        Ok((peer_id, protocol))
    }

    /// Sends a message over the stream (delegates to stream module).
    pub(super) async fn send(
        &self,
        stream: &mut Stream,
        message: &StreamMessage<'_>,
        shared_key: Option<(SharedKey, Nonce)>,
    ) -> eyre::Result<()> {
        super::stream::send(stream, message, shared_key).await
    }

    /// Receives a message from the stream (delegates to stream module).
    pub(super) async fn recv(
        &self,
        stream: &mut Stream,
        shared_key: Option<(SharedKey, Nonce)>,
    ) -> eyre::Result<Option<StreamMessage<'static>>> {
        let budget = self.sync_config.timeout / 3;
        super::stream::recv(stream, shared_key, budget).await
    }

    async fn initiate_sync_inner(
        &self,
        context_id: ContextId,
        chosen_peer: PeerId,
    ) -> eyre::Result<SyncProtocol> {
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

        // Check if we need to catch up on deltas
        let is_uninitialized = *context.root_hash == [0; 32];

        if is_uninitialized {
            info!(
                %context_id,
                %chosen_peer,
                "Node is uninitialized, requesting DAG heads from peer to catch up"
            );

            let result = self
                .dag_bootstrapper
                .catch_up(context_id, chosen_peer, our_identity, &mut stream)
                .await?;

            // If peer had no data (heads_count=0), return error to try next peer
            if matches!(result, SyncProtocol::None) {
                bail!("Peer has no data for this context");
            }

            return Ok(result);
        }

        // Check if we have pending deltas (incomplete DAG)
        // Even if node has some state, it might be missing parent deltas
        if let Some(delta_store) = self.node_state.delta_stores.get(&context_id) {
            let missing_result = delta_store.get_missing_parents().await;

            // Note: Cascaded events from DB loads are handled in state_delta handler
            if !missing_result.cascaded_events.is_empty() {
                info!(
                    %context_id,
                    cascaded_count = missing_result.cascaded_events.len(),
                    "Cascaded deltas from DB load (handlers executed in state_delta path)"
                );
            }

            if !missing_result.missing_ids.is_empty() {
                warn!(
                    %context_id,
                    %chosen_peer,
                    missing_count = missing_result.missing_ids.len(),
                    "Node has incomplete DAG (pending deltas), requesting DAG heads to catch up"
                );

                // Request DAG heads just like uninitialized nodes
                let result = self
                    .dag_bootstrapper
                    .catch_up(context_id, chosen_peer, our_identity, &mut stream)
                    .await?;

                // If peer had no data, return error to try next peer
                if matches!(result, SyncProtocol::None) {
                    bail!("Peer has no data for this context");
                }

                return Ok(result);
            }
        }

        // Otherwise, DAG-based sync happens automatically via BroadcastMessage::StateDelta
        debug!(%context_id, "Node is in sync, no active protocol needed");
        Ok(SyncProtocol::None)
    }

    pub async fn handle_opened_stream(&self, stream: Box<Stream>) {
        self.stream_responder.handle_opened_stream(stream).await;
    }
}
