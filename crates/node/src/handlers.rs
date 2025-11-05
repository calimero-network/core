//! Event handlers for network and node messages
//!
//! **CLEAN**: All handlers just dispatch to calimero-protocols!
//! No logic here - just routing and actor glue.

use std::sync::Arc;
use std::time::Duration;

use actix::{ActorFutureExt, ActorResponse, AsyncContext, Handler, Message, WrapFuture};
use calimero_network_primitives::messages::NetworkEvent;
use calimero_network_primitives::stream::{Stream, CALIMERO_BLOB_PROTOCOL};
use calimero_node_primitives::messages::get_blob_bytes::{
    GetBlobBytesRequest, GetBlobBytesResponse,
};
use calimero_node_primitives::messages::NodeMessage;
use calimero_node_primitives::sync::{BroadcastMessage, InitPayload, StreamMessage as SyncMessage};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_utils_actix::adapters::ActorExt;
use futures_util::{io, StreamExt, TryStreamExt};
use libp2p::{PeerId, StreamProtocol};
use tracing::{debug, error, info, warn};

use crate::NodeManager;

// ═══════════════════════════════════════════════════════════════════════════
// NodeMessage Handler
// ═══════════════════════════════════════════════════════════════════════════

impl Handler<NodeMessage> for NodeManager {
    type Result = ();

    fn handle(&mut self, msg: NodeMessage, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            NodeMessage::GetBlobBytes { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// GetBlobBytes Handler
// ═══════════════════════════════════════════════════════════════════════════

impl Handler<GetBlobBytesRequest> for NodeManager {
    type Result = ActorResponse<Self, <GetBlobBytesRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetBlobBytesRequest { blob_id }: GetBlobBytesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Check cache first
        if let Some(data) = self.state.blob_cache.get(&blob_id) {
            return ActorResponse::reply(Ok(GetBlobBytesResponse {
                bytes: Some(data),
            }));
        }

        // Not in cache, load from blobstore
        let blobstore = self.managers.blobstore.clone();
        let blob_cache = self.state.blob_cache.clone();

        let task = async move {
            let Some(blob) = blobstore.get(blob_id)? else {
                return Ok(GetBlobBytesResponse { bytes: None });
            };

            let mut blob = blob.map_err(io::Error::other).into_async_read();
            let mut bytes = Vec::new();
            let _ignored = io::copy(&mut blob, &mut bytes).await?;

            let data: std::sync::Arc<[u8]> = bytes.into();
            blob_cache.put(blob_id, data.clone());

            Ok(GetBlobBytesResponse { bytes: Some(data) })
        };

        ActorResponse::r#async(task.into_actor(self).map(move |res, _act, _ctx| res))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// NetworkEvent Handler
// ═══════════════════════════════════════════════════════════════════════════

impl Handler<NetworkEvent> for NodeManager {
    type Result = <NetworkEvent as actix::Message>::Result;

    fn handle(&mut self, msg: NetworkEvent, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            // Simple events - just logging
            NetworkEvent::ListeningOn { address, .. } => {
                info!("Listening on: {}", address);
            }

            NetworkEvent::Subscribed { peer_id, topic } => {
                let Ok(context_id): Result<ContextId, _> = topic.as_str().parse() else {
                    return;
                };

                if !self
                    .clients
                    .context
                    .has_context(&context_id)
                    .unwrap_or_default()
                {
                    debug!(%context_id, %peer_id, "Observed subscription to unknown context");
                    return;
                }

                info!("Peer '{}' subscribed to context '{}'", peer_id, context_id);
            }

            NetworkEvent::Unsubscribed { peer_id, topic } => {
                let Ok(context_id): Result<ContextId, _> = topic.as_str().parse() else {
                    return;
                };
                info!("Peer '{}' unsubscribed from context '{}'", peer_id, context_id);
            }

            // BroadcastMessage handling - call protocols directly!
            NetworkEvent::Message { message, .. } => {
                let Some(source) = message.source else {
                    warn!(?message, "Received message without source");
                    return;
                };

                let message = match borsh::from_slice::<BroadcastMessage<'_>>(&message.data) {
                    Ok(message) => message,
                    Err(err) => {
                        debug!(?err, "Failed to deserialize message");
                        return;
                    }
                };

                match message {
                    BroadcastMessage::StateDelta {
                        context_id,
                        author_id,
                        delta_id,
                        parent_ids,
                        hlc,
                        root_hash,
                        artifact,
                        nonce,
                        events,
                    } => {
                        handle_state_delta_broadcast(self, ctx, source, context_id, author_id, delta_id, parent_ids, hlc, root_hash, artifact, nonce, events);
                    }
                    BroadcastMessage::HashHeartbeat {
                        context_id,
                        root_hash: their_root_hash,
                        dag_heads: their_dag_heads,
                    } => {
                        handle_hash_heartbeat(self, ctx, source, context_id, their_root_hash, their_dag_heads);
                    }
                    _ => {
                        warn!(?message, "Unexpected broadcast message type");
                    }
                }
            }

            // Stream routing
            NetworkEvent::StreamOpened {
                peer_id,
                stream,
                protocol,
            } => {
                handle_stream_opened(self, ctx, peer_id, stream, protocol);
            }

            // Blob events - just logging
            NetworkEvent::BlobRequested { blob_id, context_id, requesting_peer } => {
                debug!(%blob_id, %context_id, %requesting_peer, "Blob requested by peer");
            }
            NetworkEvent::BlobProvidersFound { blob_id, context_id, providers } => {
                debug!(%blob_id, context_id = ?context_id, providers_count = providers.len(), "Blob providers found");
            }
            NetworkEvent::BlobDownloaded { blob_id, context_id, data, from_peer } => {
                handle_blob_downloaded(self, ctx, blob_id, context_id, data.into(), from_peer);
            }
            NetworkEvent::BlobDownloadFailed { blob_id, context_id, from_peer, error } => {
                info!(%blob_id, %context_id, %from_peer, %error, "Blob download failed");
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// State Delta Broadcast Handler
// ═══════════════════════════════════════════════════════════════════════════

#[allow(clippy::too_many_arguments)]
fn handle_state_delta_broadcast(
    node_manager: &mut NodeManager,
    ctx: &mut <NodeManager as actix::Actor>::Context,
    source: PeerId,
    context_id: ContextId,
    author_id: calimero_primitives::identity::PublicKey,
    delta_id: [u8; 32],
    parent_ids: Vec<[u8; 32]>,
    hlc: calimero_storage::logical_clock::HybridTimestamp,
    root_hash: calimero_primitives::hash::Hash,
    artifact: std::borrow::Cow<'_, [u8]>,
    nonce: calimero_crypto::Nonce,
    events: Option<std::borrow::Cow<'_, [u8]>>,
) {
    info!(%context_id, %author_id, delta_id = ?delta_id, "StateDelta broadcast");

    // Convert Cow to owned before moving into async
    let artifact = artifact.into_owned();
    let events = events.map(|e| e.into_owned());

    let context_client = node_manager.clients.context.clone();
    let node_client = node_manager.clients.node.clone();
    let node_state = node_manager.state.clone();
    let network_client = node_manager.managers.network.clone();
    let sync_timeout = node_manager.managers.sync_timeout;

    let _ignored = ctx.spawn(
        async move {
            // Get our identity
            let identities = context_client.get_context_members(&context_id, Some(true));
            let Some((our_identity, _)) = crate::utils::choose_stream(identities, &mut rand::thread_rng())
                .await
                .transpose()
                .ok()
                .flatten()
            else {
                warn!(%context_id, "No owned identities for context");
                return;
            };

            // Get or create DeltaStore
            let (delta_store, is_new) = node_state.delta_stores.get_or_create_with(&context_id, || {
                crate::delta_store::DeltaStore::new([0u8; 32], context_client.clone(), context_id, our_identity)
            });
            let delta_store = delta_store.clone();

            if is_new {
                if let Err(e) = delta_store.load_persisted_deltas().await {
                    warn!(?e, %context_id, "Failed to load persisted deltas");
                }
            }

            // Call protocol directly!
            if let Err(err) = calimero_protocols::gossipsub::state_delta::handle_state_delta(
                &node_client,
                &context_client,
                &network_client,
                &delta_store,
                our_identity,
                sync_timeout,
                source,
                context_id,
                author_id,
                delta_id,
                parent_ids,
                hlc,
                root_hash,
                artifact,
                nonce,
                events,
            )
            .await
            {
                warn!(?err, "Failed to handle state delta");
            }
        }
        .into_actor(node_manager),
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Hash Heartbeat Handler
// ═══════════════════════════════════════════════════════════════════════════

fn handle_hash_heartbeat(
    node_manager: &mut NodeManager,
    ctx: &mut <NodeManager as actix::Actor>::Context,
    source: PeerId,
    context_id: ContextId,
    their_root_hash: calimero_primitives::hash::Hash,
    their_dag_heads: Vec<[u8; 32]>,
) {
    let context_client = node_manager.clients.context.clone();

    if let Ok(Some(our_context)) = context_client.get_context(&context_id) {
        let our_heads_set: std::collections::HashSet<_> = our_context.dag_heads.iter().collect();
        let their_heads_set: std::collections::HashSet<_> = their_dag_heads.iter().collect();

        // Divergence detection
        if our_heads_set == their_heads_set && our_context.root_hash != their_root_hash {
            error!(
                %context_id,
                ?source,
                our_hash = ?our_context.root_hash,
                their_hash = ?their_root_hash,
                "DIVERGENCE DETECTED!"
            );
            warn!(%context_id, "Divergence detected - periodic sync will recover");
        } else if our_context.root_hash != their_root_hash {
            let heads_we_dont_have: Vec<_> = their_heads_set.difference(&our_heads_set).collect();

            if !heads_we_dont_have.is_empty() {
                info!(
                    %context_id,
                    missing_count = heads_we_dont_have.len(),
                    "Peer has DAG heads we don't have - triggering sync"
                );

                let node_client = node_manager.clients.node.clone();
                let _ignored = ctx.spawn(
                    async move {
                        if let Err(e) = node_client.sync(Some(&context_id), None).await {
                            warn!(%context_id, ?e, "Failed to trigger sync from heartbeat");
                        }
                    }
                    .into_actor(node_manager),
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Blob Downloaded Handler
// ═══════════════════════════════════════════════════════════════════════════

fn handle_blob_downloaded(
    node_manager: &mut NodeManager,
    ctx: &mut <NodeManager as actix::Actor>::Context,
    blob_id: BlobId,
    context_id: ContextId,
    data: std::sync::Arc<[u8]>,
    from_peer: PeerId,
) {
    info!(%blob_id, %context_id, %from_peer, data_size = data.len(), "Blob downloaded");

    let blobstore = node_manager.managers.blobstore.clone();
    let _ignored = ctx.spawn(
        async move {
            let reader = &data[..];
            match blobstore.put(reader).await {
                Ok((stored_blob_id, _hash, size)) => {
                    info!(%blob_id, %stored_blob_id, %size, "Blob stored successfully");
                }
                Err(e) => {
                    error!(%blob_id, error = %e, "Failed to store downloaded blob");
                }
            }
        }
        .into_actor(node_manager),
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Stream Routing
// ═══════════════════════════════════════════════════════════════════════════

fn handle_stream_opened(
    node_manager: &mut NodeManager,
    ctx: &mut <NodeManager as actix::Actor>::Context,
    peer_id: PeerId,
    mut stream: Box<Stream>,
    protocol: StreamProtocol,
) {
    if protocol == CALIMERO_BLOB_PROTOCOL {
        info!(%peer_id, "Routing to blob protocol");
        let node_client = node_manager.clients.node.clone();
        let _ignored = ctx.spawn(
            async move {
                // Call protocol directly!
                if let Err(err) = calimero_protocols::p2p::blob_protocol::handle_blob_protocol_stream(
                    &node_client,
                    peer_id,
                    &mut stream,
                ).await {
                    debug!(%peer_id, error = %err, "Failed to handle blob protocol");
                }
            }
            .into_actor(node_manager),
        );
    } else {
        // Sync protocol - call protocols directly!
        debug!(%peer_id, "Routing to sync protocol");

        let context_client = node_manager.clients.context.clone();
        let node_client = node_manager.clients.node.clone();
        let network_client = node_manager.managers.network.clone();
        let sync_timeout = node_manager.managers.sync_timeout;
        let node_state = node_manager.state.clone();

        let _ignored = ctx.spawn(
            async move {
                if let Err(err) = handle_sync_stream(
                    context_client,
                    node_client,
                    network_client,
                    node_state,
                    sync_timeout,
                    stream,
                )
                .await
                {
                    warn!(%peer_id, error = %err, "Failed to handle sync stream");
                }
            }
            .into_actor(node_manager),
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Sync Stream Handler (calls protocols!)
// ═══════════════════════════════════════════════════════════════════════════

async fn handle_sync_stream(
    context_client: calimero_context_primitives::client::ContextClient,
    node_client: calimero_node_primitives::client::NodeClient,
    network_client: calimero_network_primitives::client::NetworkClient,
    node_state: crate::NodeState,
    sync_timeout: Duration,
    mut stream: Box<Stream>,
) -> eyre::Result<()> {
    // Read Init message
    let message_result = tokio::time::timeout(sync_timeout, stream.try_next()).await??;
    let Some(message) = message_result else {
        eyre::bail!("Connection closed before Init");
    };

    let init_msg: SyncMessage = borsh::from_slice(&message.data)?;

    match init_msg {
        SyncMessage::Init {
            context_id,
            party_id: their_identity,
            payload,
            next_nonce,
        } => {
            let Some(context) = context_client.get_context(&context_id)? else {
                eyre::bail!("Context not found: {}", context_id);
            };

            let identities = context_client.get_context_members(&context_id, Some(true));
            let Some((our_identity, _)) = crate::utils::choose_stream(identities, &mut rand::thread_rng())
                .await
                .transpose()?
            else {
                eyre::bail!("No owned identities for context: {}", context_id);
            };

            // Dispatch to protocols!
            match payload {
                InitPayload::KeyShare => {
                    info!(%context_id, "KeyShare → calimero_protocols");
                    calimero_protocols::p2p::key_exchange::handle_key_exchange(
                        &mut stream,
                        &context,
                        our_identity,
                        their_identity,
                        next_nonce,
                        &context_client,
                        sync_timeout,
                    )
                    .await?;
                }

                InitPayload::DeltaRequest { delta_id, .. } => {
                    info!(%context_id, "DeltaRequest → calimero_protocols");
                    let delta_store_opt = node_state.delta_stores.get(&context_id).map(|r| r.clone());
                    let handle = context_client.datastore_handle();

                    calimero_protocols::p2p::delta_request::handle_delta_request(
                        &mut stream,
                        context_id,
                        delta_id,
                        their_identity,
                        our_identity,
                        &handle,
                        delta_store_opt
                            .as_ref()
                            .map(|s| s as &dyn calimero_protocols::p2p::delta_request::DeltaStore),
                        &context_client,
                        sync_timeout,
                    )
                    .await?;
                }

                InitPayload::BlobShare { blob_id } => {
                    info!(%context_id, "BlobShare → calimero_protocols");
                    calimero_protocols::p2p::blob_request::handle_blob_request(
                        &mut stream,
                        &context,
                        our_identity,
                        their_identity,
                        blob_id,
                        &node_client,
                        &context_client,
                        sync_timeout,
                    )
                    .await?;
                }

                InitPayload::DagHeadsRequest { .. } => {
                    info!(%context_id, "DagHeadsRequest → calimero_protocols");
                    calimero_protocols::p2p::delta_request::handle_dag_heads_request(
                        &mut stream,
                        context_id,
                        their_identity,
                        our_identity,
                        &context_client,
                        sync_timeout,
                    )
                    .await?;
                }
            }

            Ok(())
        }
        _ => eyre::bail!("Expected Init message"),
    }
}