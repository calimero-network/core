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
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::common::DIGEST_SIZE;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use eyre::bail;
use eyre::WrapErr;
use futures_util::stream::{self, FuturesUnordered};
use futures_util::{FutureExt, StreamExt};
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;
use rand::seq::SliceRandom;
use rand::Rng;
use tokio::sync::mpsc;
use tokio::time::{self, timeout_at, Instant, MissedTickBehavior};
use tracing::{debug, error, info, warn};

use crate::utils::choose_stream;

use super::config::SyncConfig;
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

    pub(super) ctx_sync_rx: Option<mpsc::Receiver<(Option<ContextId>, Option<PeerId>)>>,
}

impl Clone for SyncManager {
    fn clone(&self) -> Self {
        Self {
            sync_config: self.sync_config,
            node_client: self.node_client.clone(),
            context_client: self.context_client.clone(),
            network_client: self.network_client.clone(),
            node_state: self.node_state.clone(),
            ctx_sync_rx: None, // Receiver can't be cloned
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
        Self {
            sync_config,
            node_client,
            context_client,
            network_client,
            node_state,
            ctx_sync_rx: Some(ctx_sync_rx),
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

                    // CRITICAL FIX: Drain all other pending sync requests in the queue.
                    // When multiple contexts join rapidly (common in E2E tests), they all
                    // call sync() which queues requests in ctx_sync_rx. The old code only
                    // processed ONE request per loop iteration, leaving contexts 2-N queued
                    // indefinitely. This caused those contexts to never sync and remain
                    // with dag_heads=[] and Uninitialized errors.
                    //
                    // Solution: Use try_recv() to drain all buffered requests immediately,
                    // then trigger a full sync that will process all contexts.
                    let mut drained_count = 0;
                    while ctx_sync_rx.try_recv().is_ok() {
                        drained_count += 1;
                    }

                    if drained_count > 0 {
                        info!(drained_count, "Drained additional sync requests from queue, will sync all contexts");
                        // Clear requested_ctx to force syncing ALL contexts
                        // This ensures newly-joined contexts get synced even if they weren't first in queue
                        requested_ctx = None;
                        requested_peer = None;
                    }
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
        if let Some(peer_id) = peer_id {
            return self.initiate_sync(context_id, peer_id).await;
        }

        // CRITICAL FIX: Retry peer discovery if mesh is still forming
        // After subscribing to a context, gossipsub needs time to form the mesh.
        // We retry a few times with short delays to handle this gracefully.
        let mut peers = Vec::new();
        for attempt in 1..=3 {
            peers = self
                .network_client
                .mesh_peers(TopicHash::from_raw(context_id))
                .await;

            if !peers.is_empty() {
                break;
            }

            if attempt < 3 {
                debug!(
                    %context_id,
                    attempt,
                    "No peers found yet, mesh may still be forming, retrying..."
                );
                time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }

        if peers.is_empty() {
            bail!("No peers to sync with for context {}", context_id);
        }

        // Check if we're uninitialized
        let context = self
            .context_client
            .get_context(&context_id)?
            .ok_or_else(|| eyre::eyre!("Context not found: {}", context_id))?;

        let is_uninitialized = *context.root_hash == [0; 32];

        if is_uninitialized {
            // When uninitialized, we need to bootstrap from a peer that HAS data
            // Trying random peers can result in querying other uninitialized nodes
            info!(
                %context_id,
                peer_count = peers.len(),
                "Node is uninitialized, selecting peer with state for bootstrapping"
            );

            // Try to find a peer with actual state
            match self.find_peer_with_state(context_id, &peers).await {
                Ok(peer_id) => {
                    info!(%context_id, %peer_id, "Found peer with state, syncing from them");
                    return self.initiate_sync(context_id, peer_id).await;
                }
                Err(e) => {
                    warn!(%context_id, error = %e, "Failed to find peer with state, falling back to random selection");
                    // Fall through to random selection
                }
            }
        }

        // Normal sync: try all peers until we find one that works
        // (for initialized nodes or fallback when we can't find a peer with state)
        debug!(%context_id, "Using random peer selection for sync");
        for peer_id in peers.choose_multiple(&mut rand::thread_rng(), peers.len()) {
            if let Ok(result) = self.initiate_sync(context_id, *peer_id).await {
                return Ok(result);
            }
        }

        bail!("Failed to sync with any peer for context {}", context_id)
    }

    /// Find a peer that has state (non-zero root_hash and non-empty DAG heads)
    ///
    /// This is critical for bootstrapping newly joined nodes. Without this,
    /// uninitialized nodes may query other uninitialized nodes, resulting in
    /// all nodes remaining uninitialized.
    async fn find_peer_with_state(
        &self,
        context_id: ContextId,
        peers: &[PeerId],
    ) -> eyre::Result<PeerId> {
        use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};

        // Get our identity for handshake
        let identities = self
            .context_client
            .get_context_members(&context_id, Some(true));

        let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
            .await
            .transpose()?
        else {
            bail!("no owned identities found for context: {}", context_id);
        };

        // Query peers to find one with state
        for peer_id in peers {
            debug!(%context_id, %peer_id, "Querying peer for state");

            // Try to open stream and request DAG heads
            let stream_result = self.network_client.open_stream(*peer_id).await;
            let mut stream = match stream_result {
                Ok(s) => s,
                Err(e) => {
                    debug!(%context_id, %peer_id, error = %e, "Failed to open stream to peer");
                    continue;
                }
            };

            // Send DAG heads request
            let request_msg = StreamMessage::Init {
                context_id,
                party_id: our_identity,
                payload: InitPayload::DagHeadsRequest { context_id },
                next_nonce: {
                    use rand::Rng;
                    rand::thread_rng().gen()
                },
            };

            if let Err(e) = self.send(&mut stream, &request_msg, None).await {
                debug!(%context_id, %peer_id, error = %e, "Failed to send DAG heads request");
                continue;
            }

            // Receive response with short timeout
            let timeout_budget = self.sync_config.timeout / 6;
            let response = match super::stream::recv(&mut stream, None, timeout_budget).await {
                Ok(Some(resp)) => resp,
                Ok(None) => {
                    debug!(%context_id, %peer_id, "No response from peer");
                    continue;
                }
                Err(e) => {
                    debug!(%context_id, %peer_id, error = %e, "Failed to receive response");
                    continue;
                }
            };

            // Check if peer has state
            if let StreamMessage::Message {
                payload:
                    MessagePayload::DagHeadsResponse {
                        dag_heads,
                        root_hash,
                    },
                ..
            } = response
            {
                // Peer has state if root_hash is not zeros
                // (even if dag_heads is empty due to migration/legacy contexts)
                let has_state = *root_hash != [0; 32];

                debug!(
                    %context_id,
                    %peer_id,
                    heads_count = dag_heads.len(),
                    %root_hash,
                    has_state,
                    "Received DAG heads from peer"
                );

                if has_state {
                    info!(
                        %context_id,
                        %peer_id,
                        heads_count = dag_heads.len(),
                        %root_hash,
                        "Found peer with state for bootstrapping"
                    );
                    return Ok(*peer_id);
                }
            }
        }

        bail!("No peers with state found for context {}", context_id)
    }

    async fn initiate_sync(
        &self,
        context_id: ContextId,
        peer_id: PeerId,
    ) -> eyre::Result<(PeerId, SyncProtocol)> {
        let start = Instant::now();

        info!(%context_id, %peer_id, "Attempting to sync with peer");

        let protocol = match self.initiate_sync_inner(context_id, peer_id).await {
            Ok(protocol) => protocol,
            Err(err) => {
                warn!(
                    %context_id,
                    %peer_id,
                    error = %err,
                    "Sync attempt failed for peer"
                );
                return Err(err);
            }
        };

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

    /// Get blob ID and application config from application or context config
    async fn get_blob_info(
        &self,
        context_id: &ContextId,
        application: &Option<calimero_primitives::application::Application>,
    ) -> eyre::Result<(
        calimero_primitives::blobs::BlobId,
        Option<calimero_primitives::application::Application>,
    )> {
        if let Some(ref app) = application {
            Ok((app.blob.bytecode, None))
        } else {
            // Application not found - get blob_id from context config
            let context_config = self
                .context_client
                .context_config(context_id)?
                .ok_or_else(|| eyre::eyre!("context config not found"))?;
            let external_client = self
                .context_client
                .external_client(context_id, &context_config)?;
            let config_client = external_client.config();
            let app_config = config_client.application().await?;
            Ok((app_config.blob.bytecode, Some(app_config)))
        }
    }

    /// Get application size from application, cached config, or context config
    async fn get_application_size(
        &self,
        context_id: &ContextId,
        application: &Option<calimero_primitives::application::Application>,
        app_config_opt: &Option<calimero_primitives::application::Application>,
    ) -> eyre::Result<u64> {
        if let Some(ref app) = application {
            Ok(app.size)
        } else if let Some(ref app_config) = app_config_opt {
            Ok(app_config.size)
        } else {
            let context_config = self
                .context_client
                .context_config(context_id)?
                .ok_or_else(|| eyre::eyre!("context config not found"))?;
            let external_client = self
                .context_client
                .external_client(context_id, &context_config)?;
            let config_client = external_client.config();
            let app_config = config_client.application().await?;
            Ok(app_config.size)
        }
    }

    /// Get application source from cached config or context config
    async fn get_application_source(
        &self,
        context_id: &ContextId,
        app_config_opt: &Option<calimero_primitives::application::Application>,
    ) -> eyre::Result<calimero_primitives::application::ApplicationSource> {
        if let Some(ref app_config) = app_config_opt {
            Ok(app_config.source.clone())
        } else {
            let context_config = self
                .context_client
                .context_config(context_id)?
                .ok_or_else(|| eyre::eyre!("context config not found"))?;
            let external_client = self
                .context_client
                .external_client(context_id, &context_config)?;
            let config_client = external_client.config();
            let app_config = config_client.application().await?;
            Ok(app_config.source.clone())
        }
    }

    /// Install bundle application after blob sharing completes.
    ///
    /// Returns `Some(installed_application)` if a bundle was installed,
    /// `None` otherwise. Updates `context.application_id` if the installed
    /// ApplicationId differs from the context's ApplicationId.
    async fn install_bundle_after_blob_sharing(
        &self,
        context_id: &ContextId,
        blob_id: &calimero_primitives::blobs::BlobId,
        app_config_opt: &Option<calimero_primitives::application::Application>,
        context: &mut calimero_primitives::context::Context,
        application: &mut Option<calimero_primitives::application::Application>,
    ) -> eyre::Result<()> {
        // Only proceed if blob is now available locally
        if !self.node_client.has_blob(blob_id)? {
            return Ok(());
        }

        // Check if blob is a bundle
        let Some(blob_bytes) = self.node_client.get_blob_bytes(blob_id, None).await? else {
            return Ok(());
        };

        // Wrap blocking I/O in spawn_blocking to avoid blocking async runtime
        let blob_bytes_clone = blob_bytes.clone();
        let is_bundle =
            tokio::task::spawn_blocking(move || NodeClient::is_bundle_blob(&blob_bytes_clone))
                .await?;

        if !is_bundle {
            return Ok(());
        }

        // Get source from context config (use cached if available, otherwise fetch)
        let source = self
            .get_application_source(context_id, app_config_opt)
            .await?;

        // Install bundle
        let installed_app_id = self
            .node_client
            .install_application_from_bundle_blob(blob_id, &source)
            .await
            .map_err(|e| {
                eyre::eyre!(
                    "Failed to install bundle application from blob {}: {}",
                    blob_id,
                    e
                )
            })?;

        // Verify installation succeeded by fetching the installed application
        let installed_application = self
            .node_client
            .get_application(&installed_app_id)
            .map_err(|e| {
                eyre::eyre!(
                    "Failed to verify bundle installation for application {}: {}",
                    installed_app_id,
                    e
                )
            })?;

        let Some(installed_application) = installed_application else {
            bail!(
                "Bundle installation reported success but application {} is not retrievable",
                installed_app_id
            );
        };

        // Check if the installed ApplicationId matches the context's ApplicationId
        if installed_app_id != context.application_id {
            warn!(
                installed_app_id = %installed_app_id,
                context_app_id = %context.application_id,
                "Installed application ID does not match context application ID, updating to installed ID"
            );
            // Update context with the installed application ID for consistency
            context.application_id = installed_app_id;

            // Persist the ApplicationId change to the database
            // This is critical: if we don't persist, the old ApplicationId will be
            // used on node restart, causing application lookup failures
            self.context_client
                .update_context_application_id(context_id, installed_app_id)
                .map_err(|e| {
                    eyre::eyre!(
                        "Failed to persist ApplicationId update for context {}: {}",
                        context_id,
                        e
                    )
                })?;

            debug!(
                %context_id,
                installed_app_id = %installed_app_id,
                "Persisted ApplicationId update to database"
            );
        }

        // Use the verified installed application
        *application = Some(installed_application);

        Ok(())
    }

    /// Handle DAG synchronization for uninitialized nodes or nodes with incomplete DAGs
    async fn handle_dag_sync(
        &self,
        context_id: ContextId,
        context: &calimero_primitives::context::Context,
        chosen_peer: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<Option<SyncProtocol>> {
        let is_uninitialized = *context.root_hash == [0; 32];

        // Check for incomplete sync from a previous run (crash recovery)
        let has_incomplete_sync = self.check_sync_in_progress(context_id)?.is_some();
        if has_incomplete_sync {
            warn!(
                %context_id,
                "Detected incomplete snapshot sync from previous run, forcing re-sync"
            );
        }

        if is_uninitialized || has_incomplete_sync {
            info!(
                %context_id,
                %chosen_peer,
                is_uninitialized,
                has_incomplete_sync,
                "Node needs snapshot sync, checking if peer has state"
            );

            // Query peer's state to decide sync strategy
            let peer_state = self
                .query_peer_dag_state(context_id, chosen_peer, our_identity, stream)
                .await?;

            match peer_state {
                Some((peer_root_hash, _peer_dag_heads)) if *peer_root_hash != [0; 32] => {
                    // Peer has state - use snapshot sync for efficient bootstrap
                    info!(
                        %context_id,
                        %chosen_peer,
                        peer_root_hash = %peer_root_hash,
                        "Peer has state, using snapshot sync for bootstrap"
                    );

                    // Note: request_snapshot_sync opens its own stream, existing stream
                    // will be closed when this function returns
                    // force=false: This is bootstrap for uninitialized nodes
                    match self
                        .request_snapshot_sync(context_id, chosen_peer, false)
                        .await
                        .wrap_err("snapshot sync")
                    {
                        Ok(result) => {
                            info!(
                                %context_id,
                                %chosen_peer,
                                applied_records = result.applied_records,
                                boundary_root_hash = %result.boundary_root_hash,
                                dag_heads_count = result.dag_heads.len(),
                                "Snapshot sync completed successfully"
                            );

                            // CRITICAL: Add snapshot boundary checkpoints to DAG
                            // This ensures that when new deltas arrive referencing the
                            // snapshot boundary heads as parents, the DAG accepts them.
                            if !result.dag_heads.is_empty() {
                                let delta_store = self
                                    .node_state
                                    .delta_stores
                                    .entry(context_id)
                                    .or_insert_with(|| {
                                        crate::delta_store::DeltaStore::new(
                                            [0u8; 32],
                                            self.context_client.clone(),
                                            context_id,
                                            our_identity,
                                        )
                                    })
                                    .clone();

                                let checkpoints_added = delta_store
                                    .add_snapshot_checkpoints(
                                        result.dag_heads.clone(),
                                        *result.boundary_root_hash,
                                    )
                                    .await;

                                info!(
                                    %context_id,
                                    checkpoints_added,
                                    "Added snapshot boundary checkpoints to DAG"
                                );

                                match self.network_client.open_stream(chosen_peer).await {
                                    Ok(mut fine_stream) => {
                                        if let Err(e) = self
                                            .fine_sync_from_boundary(
                                                context_id,
                                                chosen_peer,
                                                our_identity,
                                                &mut fine_stream,
                                            )
                                            .await
                                        {
                                            warn!(
                                                %context_id,
                                                %chosen_peer,
                                                error = %e,
                                                "Fine-sync after snapshot failed, state may be slightly behind"
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        warn!(
                                            %context_id,
                                            %chosen_peer,
                                            error = %e,
                                            "Fine-sync stream open failed, state may be slightly behind"
                                        );
                                    }
                                }
                            }

                            // Replay any buffered deltas (from uninitialized context period)
                            // This ensures handlers execute for deltas that arrived before sync completed
                            if let Some(buffered_deltas) =
                                self.node_state.end_sync_session(&context_id)
                            {
                                let buffered_count = buffered_deltas.len();
                                if buffered_count > 0 {
                                    info!(
                                        %context_id,
                                        buffered_count,
                                        "Replaying buffered deltas after snapshot sync (bootstrap path)"
                                    );
                                    self.replay_buffered_deltas(
                                        context_id,
                                        our_identity,
                                        buffered_deltas,
                                        chosen_peer,
                                    )
                                    .await;
                                }
                            }

                            return Ok(Some(SyncProtocol::SnapshotSync));
                        }
                        Err(e) => {
                            warn!(
                                %context_id,
                                %chosen_peer,
                                error = %e,
                                "Snapshot sync failed, will retry with another peer"
                            );
                            bail!("Snapshot sync failed: {}", e);
                        }
                    }
                }
                Some(_) => {
                    // Peer is also uninitialized, try next peer
                    info!(%context_id, %chosen_peer, "Peer also has no state, trying next peer");
                    bail!("Peer has no data for this context");
                }
                None => {
                    // Failed to query peer state
                    bail!("Failed to query peer state for context {}", context_id);
                }
            }
        }

        // Check if we have pending deltas (incomplete DAG)
        // Even if node has some state, it might be missing parent deltas
        if let Some(delta_store) = self.node_state.delta_stores.get(&context_id) {
            // Reload persisted deltas to catch locally-created deltas from execute.rs
            // that are in the database but not in the in-memory DeltaStore
            let _ = delta_store.load_persisted_deltas().await;
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
                    .request_dag_heads_and_sync(context_id, chosen_peer, our_identity, stream)
                    .await
                    .wrap_err("request DAG heads and sync")?;

                // If peer had no data, return error to try next peer
                if matches!(result, SyncProtocol::None) {
                    bail!("Peer has no data for this context");
                }

                return Ok(Some(result));
            }
        }

        // Compare our state with peer's state even if we think we're in sync.
        // The peer might have new heads we don't know about (e.g., if gossipsub messages were lost).
        let peer_state = self
            .query_peer_dag_state(context_id, chosen_peer, our_identity, stream)
            .await?;

        if let Some((peer_root_hash, peer_dag_heads)) = peer_state {
            if *context.root_hash != *peer_root_hash {
                info!(
                    %context_id,
                    %chosen_peer,
                    our_root_hash = %context.root_hash,
                    peer_root_hash = %peer_root_hash,
                    our_heads_count = context.dag_heads.len(),
                    peer_heads_count = peer_dag_heads.len(),
                    "Root hash mismatch with peer, triggering DAG catchup"
                );

                let our_heads_set: std::collections::HashSet<_> =
                    context.dag_heads.iter().collect();
                let peer_heads_set: std::collections::HashSet<_> = peer_dag_heads.iter().collect();

                // Heads peer has that we don't have
                let missing_from_peer: Vec<_> = peer_dag_heads
                    .iter()
                    .filter(|h| !our_heads_set.contains(h))
                    .cloned()
                    .collect();

                // Heads we have that peer doesn't have
                let peer_missing: Vec<_> = context
                    .dag_heads
                    .iter()
                    .filter(|h| !peer_heads_set.contains(h))
                    .cloned()
                    .collect();

                if !missing_from_peer.is_empty() {
                    // Peer has heads we don't have, request them
                    info!(
                        %context_id,
                        %chosen_peer,
                        missing_count = missing_from_peer.len(),
                        "Peer has DAG heads we don't have, requesting them"
                    );

                    let result = self
                        .request_dag_heads_and_sync(context_id, chosen_peer, our_identity, stream)
                        .await
                        .wrap_err("request DAG heads and sync")?;

                    // If peer had no data or unexpected response, return error to try next peer
                    if matches!(result, SyncProtocol::None) {
                        bail!("Peer has no data or unexpected response for this context, will try next peer");
                    }

                    return Ok(Some(result));
                } else if !peer_missing.is_empty() {
                    // We have heads peer doesn't have - we are ahead, no need to sync FROM them
                    // The peer will request from us when they sync
                    info!(
                        %context_id,
                        %chosen_peer,
                        our_extra_heads = peer_missing.len(),
                        "We have DAG heads peer doesn't have - we are ahead, skipping sync from this peer"
                    );
                    // Return None to indicate no sync needed from this peer
                    return Ok(None);
                } else {
                    // Truly same heads but different root hash - this is a state divergence
                    // that can only be resolved via snapshot sync (DAG sync won't help
                    // since there are no missing deltas on either side)
                    warn!(
                        %context_id,
                        %chosen_peer,
                        our_root_hash = %context.root_hash,
                        peer_root_hash = %peer_root_hash,
                        our_heads_count = context.dag_heads.len(),
                        peer_heads_count = peer_dag_heads.len(),
                        "STATE DIVERGENCE: Truly same DAG heads but different root hash - forcing snapshot sync"
                    );

                    // Force snapshot sync to reconcile state
                    let result = self
                        .fallback_to_snapshot_sync(context_id, our_identity, chosen_peer, stream)
                        .await
                        .wrap_err("snapshot sync for state divergence")?;

                    return Ok(Some(result));
                }
            } else {
                debug!(
                    %context_id,
                    %chosen_peer,
                    root_hash = %context.root_hash,
                    "Root hash matches peer, node is truly in sync"
                );
            }
        }

        Ok(None)
    }

    /// Query peer for their DAG state (root_hash and dag_heads) without triggering full sync.
    ///
    /// Returns `Ok(Some((root_hash, dag_heads)))` if peer responded successfully,
    /// `Ok(None)` if peer had no valid response or no state, or `Err` on communication error.
    async fn query_peer_dag_state(
        &self,
        context_id: ContextId,
        chosen_peer: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<Option<(calimero_primitives::hash::Hash, Vec<[u8; DIGEST_SIZE]>)>> {
        let request_msg = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::DagHeadsRequest { context_id },
            next_nonce: rand::thread_rng().gen(),
        };

        self.send(stream, &request_msg, None).await?;

        let response = self.recv(stream, None).await?;

        match response {
            Some(StreamMessage::Message {
                payload:
                    MessagePayload::DagHeadsResponse {
                        dag_heads,
                        root_hash,
                    },
                ..
            }) => {
                debug!(
                    %context_id,
                    %chosen_peer,
                    heads_count = dag_heads.len(),
                    peer_root_hash = %root_hash,
                    "Received peer DAG state for comparison"
                );
                Ok(Some((root_hash, dag_heads)))
            }
            _ => {
                debug!(%context_id, %chosen_peer, "Failed to get peer DAG state for comparison");
                Ok(None)
            }
        }
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

        // Get application - if not found, we'll try to install it after blob sharing
        let mut application = self.node_client.get_application(&context.application_id)?;

        // Get blob_id and app config for later use
        let (blob_id, app_config_opt) = self.get_blob_info(&context_id, &application).await?;

        let identities = self
            .context_client
            .get_context_members(&context.id, Some(true));

        let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
            .await
            .transpose()?
        else {
            bail!("no owned identities found for context: {}", context.id);
        };

        let mut stream = self
            .network_client
            .open_stream(chosen_peer)
            .await
            .wrap_err("open stream for sync")?;

        self.initiate_key_share_process(&mut context, our_identity, &mut stream)
            .await
            .wrap_err("key share")?;

        if !self.node_client.has_blob(&blob_id)? {
            // Get size from application config if we don't have application yet
            let size = self
                .get_application_size(&context_id, &application, &app_config_opt)
                .await?;

            self.initiate_blob_share_process(&context, our_identity, blob_id, size, &mut stream)
                .await
                .wrap_err("blob share")?;

            // After blob sharing, try to install application if it doesn't exist
            if application.is_none() {
                self.install_bundle_after_blob_sharing(
                    &context_id,
                    &blob_id,
                    &app_config_opt,
                    &mut context,
                    &mut application,
                )
                .await
                .wrap_err("install bundle after blob share")?;
            }
        }

        let Some(_application) = application else {
            bail!("application not found: {}", context.application_id);
        };

        // Handle DAG synchronization if needed (uninitialized or incomplete DAG)
        if let Some(result) = self
            .handle_dag_sync(context_id, &context, chosen_peer, our_identity, &mut stream)
            .await
            .wrap_err("DAG sync")?
        {
            return Ok(result);
        }

        // Otherwise, DAG-based sync happens automatically via BroadcastMessage::StateDelta
        debug!(%context_id, "Node is in sync, no active protocol needed");
        Ok(SyncProtocol::None)
    }

    /// Request peer's DAG heads and sync all missing deltas
    async fn request_dag_heads_and_sync(
        &self,
        context_id: ContextId,
        peer_id: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<SyncProtocol> {
        use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};

        // Send DAG heads request
        let request_msg = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::DagHeadsRequest { context_id },
            next_nonce: {
                use rand::Rng;
                rand::thread_rng().gen()
            },
        };

        self.send(stream, &request_msg, None).await?;

        // Receive response
        let response = self.recv(stream, None).await?;

        match response {
            Some(StreamMessage::Message {
                payload:
                    MessagePayload::DagHeadsResponse {
                        dag_heads,
                        root_hash,
                    },
                ..
            }) => {
                info!(
                    %context_id,
                    heads_count = dag_heads.len(),
                    peer_root_hash = %root_hash,
                    "Received DAG heads from peer, requesting deltas"
                );

                // Check if peer has state even without DAG heads
                if dag_heads.is_empty() && *root_hash != [0; 32] {
                    error!(
                        %context_id,
                        peer_root_hash = %root_hash,
                        "Peer has state but no DAG heads!"
                    );
                    bail!(
                        "Peer has state but no DAG heads (migration issue). \
                         Clear data directories on both nodes and recreate context."
                    );
                }

                if dag_heads.is_empty() {
                    info!(%context_id, "Peer also has no deltas and no state, will try next peer");
                    // Return None to signal caller to try next peer
                    return Ok(SyncProtocol::None);
                }

                // CRITICAL FIX: Fetch ALL DAG heads first, THEN request missing parents
                // This ensures we don't miss sibling heads that might be the missing parents

                // Get or create DeltaStore for this context (do this once before the loop)
                let (delta_store_ref, is_new_store) = {
                    let mut is_new = false;
                    let delta_store = self
                        .node_state
                        .delta_stores
                        .entry(context_id)
                        .or_insert_with(|| {
                            is_new = true;
                            crate::delta_store::DeltaStore::new(
                                [0u8; 32],
                                self.context_client.clone(),
                                context_id,
                                our_identity,
                            )
                        });

                    let delta_store_ref = delta_store.clone();
                    (delta_store_ref, is_new)
                };

                // Always reload persisted deltas from database before sync operations
                // This is critical because local deltas created via execute.rs are persisted
                // to the database but NOT added to the in-memory DeltaStore. Without this
                // reload, the DeltaStore would be missing locally-created deltas.
                if let Err(e) = delta_store_ref.load_persisted_deltas().await {
                    warn!(
                        ?e,
                        %context_id,
                        "Failed to load persisted deltas, starting with empty DAG"
                    );
                }

                // Phase 1: Request and add ALL DAG heads
                for head_id in &dag_heads {
                    info!(
                        %context_id,
                        head_id = ?head_id,
                        "Requesting DAG head delta from peer"
                    );

                    let delta_request = StreamMessage::Init {
                        context_id,
                        party_id: our_identity,
                        payload: InitPayload::DeltaRequest {
                            context_id,
                            delta_id: *head_id,
                        },
                        next_nonce: {
                            use rand::Rng;
                            rand::thread_rng().gen()
                        },
                    };

                    self.send(stream, &delta_request, None).await?;

                    let delta_response = self.recv(stream, None).await?;

                    match delta_response {
                        Some(StreamMessage::Message {
                            payload: MessagePayload::DeltaResponse { delta },
                            ..
                        }) => {
                            // Deserialize and add to DAG
                            let storage_delta: calimero_storage::delta::CausalDelta =
                                borsh::from_slice(&delta)?;

                            let dag_delta = calimero_dag::CausalDelta {
                                id: storage_delta.id,
                                parents: storage_delta.parents,
                                payload: storage_delta.actions,
                                hlc: storage_delta.hlc,
                                expected_root_hash: storage_delta.expected_root_hash,
                                kind: calimero_dag::DeltaKind::Regular,
                            };

                            if let Err(e) = delta_store_ref.add_delta(dag_delta).await {
                                warn!(
                                    ?e,
                                    %context_id,
                                    head_id = ?head_id,
                                    "Failed to add DAG head delta"
                                );
                            } else {
                                info!(
                                    %context_id,
                                    head_id = ?head_id,
                                    "Successfully added DAG head delta"
                                );
                            }
                        }
                        Some(StreamMessage::Message {
                            payload:
                                MessagePayload::SnapshotError {
                                    error:
                                        calimero_node_primitives::sync::SnapshotError::SnapshotRequired,
                                },
                            ..
                        }) => {
                            info!(
                                %context_id,
                                head_id = ?head_id,
                                "Peer's delta history is pruned, falling back to snapshot sync"
                            );
                            // Fall back to snapshot sync
                            return self
                                .fallback_to_snapshot_sync(
                                    context_id,
                                    our_identity,
                                    peer_id,
                                    stream,
                                )
                                .await;
                        }
                        Some(StreamMessage::Message {
                            payload: MessagePayload::DeltaNotFound,
                            ..
                        }) => {
                            warn!(
                                %context_id,
                                head_id = ?head_id,
                                "Peer doesn't have requested DAG head delta"
                            );
                            // Continue trying other heads
                        }
                        _ => {
                            warn!(%context_id, head_id = ?head_id, "Unexpected response to delta request");
                        }
                    }
                }

                // Phase 2: Now check for missing parents and fetch them recursively
                let missing_result = delta_store_ref.get_missing_parents().await;

                // Note: Cascaded events from DB loads logged but not executed here (state_delta handler will catch them)
                if !missing_result.cascaded_events.is_empty() {
                    info!(
                        %context_id,
                        cascaded_count = missing_result.cascaded_events.len(),
                        "Cascaded deltas from DB load during DAG head sync"
                    );
                }

                if !missing_result.missing_ids.is_empty() {
                    info!(
                        %context_id,
                        missing_count = missing_result.missing_ids.len(),
                        "DAG heads have missing parents, requesting them recursively"
                    );

                    // Request missing parents (this uses recursive topological fetching)
                    if let Err(e) = self
                        .request_missing_deltas(
                            context_id,
                            missing_result.missing_ids,
                            peer_id,
                            delta_store_ref.clone(),
                            our_identity,
                        )
                        .await
                    {
                        warn!(
                            ?e,
                            %context_id,
                            "Failed to request missing parent deltas during DAG catchup"
                        );
                    }
                }

                // Return a non-None protocol to signal success (prevents trying next peer)
                Ok(SyncProtocol::DagCatchup)
            }
            _ => {
                warn!(%context_id, "Unexpected response to DAG heads request, trying next peer");
                Ok(SyncProtocol::None)
            }
        }
    }

    /// Fall back to full snapshot sync when delta sync is not possible.
    async fn fallback_to_snapshot_sync(
        &self,
        context_id: ContextId,
        our_identity: PublicKey,
        peer_id: PeerId,
        _stream: &mut Stream,
    ) -> eyre::Result<SyncProtocol> {
        info!(%context_id, %peer_id, "Initiating snapshot sync");

        // Start buffering deltas that arrive during snapshot sync
        // Use current time as sync start HLC
        let sync_start_hlc = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        self.node_state
            .start_sync_session(context_id, sync_start_hlc);

        // force=false: Enforce Invariant I5 - only allow snapshot on fresh nodes.
        // If the node has state, this will fail, which is correct - divergence
        // or pruned history on initialized nodes cannot be safely resolved via
        // snapshot overwrite. CRDT merge must be used instead.
        let result = self
            .request_snapshot_sync(context_id, peer_id, false)
            .await?;
        info!(%context_id, records = result.applied_records, "Snapshot sync completed");

        // End buffering and get any deltas that arrived during sync
        let buffered_deltas = self.node_state.end_sync_session(&context_id);
        let buffered_count = buffered_deltas.as_ref().map_or(0, Vec::len);

        if buffered_count > 0 {
            info!(
                %context_id,
                buffered_count,
                "Replaying buffered deltas after snapshot sync"
            );

            // Replay buffered deltas - now that context is initialized, we can process them
            if let Some(deltas) = buffered_deltas {
                self.replay_buffered_deltas(context_id, our_identity, deltas, peer_id)
                    .await;
            }
        }

        // Fine-sync to catch any deltas since the snapshot boundary
        if !result.dag_heads.is_empty() {
            let mut stream = self.network_client.open_stream(peer_id).await?;
            if let Err(e) = self
                .fine_sync_from_boundary(context_id, peer_id, our_identity, &mut stream)
                .await
            {
                warn!(?e, %context_id, "Fine-sync failed, state may be slightly behind");
            }
        }

        Ok(SyncProtocol::SnapshotSync)
    }

    /// Replay buffered deltas after snapshot sync completes.
    ///
    /// This ensures that:
    /// 1. Deltas arriving during sync aren't lost
    /// 2. Event handlers execute for buffered deltas
    /// 3. Ancestor deltas (whose state is covered by checkpoint) get handlers executed
    async fn replay_buffered_deltas(
        &self,
        context_id: ContextId,
        our_identity: PublicKey,
        deltas: Vec<calimero_node_primitives::delta_buffer::BufferedDelta>,
        _fallback_peer: PeerId,
    ) {
        use crate::handlers::state_delta::replay_buffered_delta;
        use std::collections::{HashMap, HashSet};

        // Build a set of IDs that are "covered" by the snapshot
        // This includes:
        // 1. Deltas that match checkpoints directly
        // 2. Deltas that are ancestors of checkpoints (their state is included in snapshot)
        let mut covered_delta_ids: HashSet<[u8; 32]> = HashSet::new();

        // Get the delta store to check for existing checkpoints
        let delta_store = self
            .node_state
            .delta_stores
            .entry(context_id)
            .or_insert_with(|| {
                crate::delta_store::DeltaStore::new(
                    [0u8; 32],
                    self.context_client.clone(),
                    context_id,
                    our_identity,
                )
            })
            .clone();

        // Build parent -> children map from buffered deltas
        let mut parent_to_children: HashMap<[u8; 32], Vec<[u8; 32]>> = HashMap::new();
        for buffered in &deltas {
            for parent in &buffered.parents {
                parent_to_children
                    .entry(*parent)
                    .or_default()
                    .push(buffered.id);
            }
        }

        // Identify which buffered deltas match existing checkpoints
        let mut checkpoint_matches: Vec<[u8; 32]> = Vec::new();
        for buffered in &deltas {
            if delta_store.dag_has_delta_applied(&buffered.id).await {
                checkpoint_matches.push(buffered.id);
                covered_delta_ids.insert(buffered.id);
            }
        }

        // Propagate "covered" status backwards through the parent chain
        // If delta D has a child C that is covered, then D is also covered
        // (D's state is included in C's checkpoint)
        let delta_ids: HashSet<[u8; 32]> = deltas.iter().map(|d| d.id).collect();
        let delta_parents: HashMap<[u8; 32], Vec<[u8; 32]>> =
            deltas.iter().map(|d| (d.id, d.parents.clone())).collect();

        // BFS backwards from checkpoint matches
        let mut queue: std::collections::VecDeque<[u8; 32]> =
            checkpoint_matches.iter().copied().collect();
        while let Some(child_id) = queue.pop_front() {
            // Get parents of this delta (if it's one of our buffered deltas)
            if let Some(parents) = delta_parents.get(&child_id) {
                for parent_id in parents {
                    // If parent is also a buffered delta and not yet covered
                    if delta_ids.contains(parent_id) && !covered_delta_ids.contains(parent_id) {
                        covered_delta_ids.insert(*parent_id);
                        queue.push_back(*parent_id);
                    }
                }
            }
        }

        if !covered_delta_ids.is_empty() {
            info!(
                %context_id,
                covered_count = covered_delta_ids.len(),
                checkpoint_matches = checkpoint_matches.len(),
                total_buffered = deltas.len(),
                "Identified buffered deltas covered by snapshot checkpoint"
            );
        }

        for buffered in deltas {
            let delta_id = buffered.id;
            let has_events = buffered.events.is_some();
            let is_covered_by_checkpoint = covered_delta_ids.contains(&delta_id);

            match replay_buffered_delta(
                &self.context_client,
                &self.node_client,
                &self.network_client,
                &self.node_state,
                context_id,
                our_identity,
                buffered,
                self.sync_config.timeout,
                is_covered_by_checkpoint,
            )
            .await
            {
                Ok(applied) => {
                    if applied {
                        info!(
                            %context_id,
                            delta_id = ?delta_id,
                            has_events,
                            "Replayed buffered delta successfully"
                        );
                    } else if is_covered_by_checkpoint {
                        debug!(
                            %context_id,
                            delta_id = ?delta_id,
                            "Buffered delta is ancestor of checkpoint (state covered, handlers executed)"
                        );
                    } else {
                        debug!(
                            %context_id,
                            delta_id = ?delta_id,
                            "Buffered delta went to pending (missing parents)"
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        %context_id,
                        delta_id = ?delta_id,
                        error = %e,
                        "Failed to replay buffered delta"
                    );
                }
            }
        }
    }

    /// Fine-sync from snapshot boundary to catch up to latest state.
    async fn fine_sync_from_boundary(
        &self,
        context_id: ContextId,
        peer_id: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        let delta_store = self
            .node_state
            .delta_stores
            .entry(context_id)
            .or_insert_with(|| {
                crate::delta_store::DeltaStore::new(
                    [0u8; 32],
                    self.context_client.clone(),
                    context_id,
                    our_identity,
                )
            })
            .clone();

        let _ = delta_store.load_persisted_deltas().await;

        let request_msg = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::DagHeadsRequest { context_id },
            next_nonce: rand::random(),
        };
        self.send(stream, &request_msg, None).await?;

        let response = self.recv(stream, None).await?;

        if let Some(StreamMessage::Message {
            payload: MessagePayload::DagHeadsResponse { dag_heads, .. },
            ..
        }) = response
        {
            let mut missing = Vec::new();
            for head in &dag_heads {
                if !delta_store.has_delta(head).await {
                    missing.push(*head);
                }
            }

            if !missing.is_empty() {
                self.request_missing_deltas(
                    context_id,
                    missing,
                    peer_id,
                    delta_store,
                    our_identity,
                )
                .await?;
            }
        }

        Ok(())
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

        let Some(context) = self.context_client.get_context(&context_id)? else {
            bail!("context not found: {}", context_id);
        };

        let mut _updated = None;

        if !self
            .context_client
            .has_member(&context_id, &their_identity)?
        {
            _updated = Some(
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

        // Note: Concurrent syncs are already prevented by SyncState tracking
        // in the start() loop. When sync starts, last_sync is set to None.
        // When complete, it's set to Some(now).

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
            // Old sync protocols removed - DAG uses gossipsub broadcast instead
            // Streams are only used for: KeyShare, BlobShare, DeltaRequest, DagHeadsRequest
            InitPayload::DeltaRequest {
                context_id: requested_context_id,
                delta_id,
            } => {
                // Handle delta request from peer
                self.handle_delta_request(requested_context_id, delta_id, stream)
                    .await?
            }
            InitPayload::DagHeadsRequest {
                context_id: requested_context_id,
            } => {
                // Handle DAG heads request from peer
                self.handle_dag_heads_request(requested_context_id, stream, nonce)
                    .await?
            }
            InitPayload::SnapshotBoundaryRequest {
                context_id: requested_context_id,
                requested_cutoff_timestamp,
            } => {
                // Handle snapshot boundary negotiation request from peer
                self.handle_snapshot_boundary_request(
                    requested_context_id,
                    requested_cutoff_timestamp,
                    stream,
                    nonce,
                )
                .await?
            }
            InitPayload::SnapshotStreamRequest {
                context_id: requested_context_id,
                boundary_root_hash,
                page_limit,
                byte_limit,
                resume_cursor,
            } => {
                // Handle snapshot stream request from peer
                self.handle_snapshot_stream_request(
                    requested_context_id,
                    boundary_root_hash,
                    page_limit,
                    byte_limit,
                    resume_cursor,
                    stream,
                    nonce,
                )
                .await?
            }
        };

        Ok(Some(()))
    }
}
