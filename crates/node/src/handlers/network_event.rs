//! Network event handlers
//!
//! **SRP Applied**: Each event type is handled in its own focused module:
//! - `state_delta.rs` - BroadcastMessage::StateDelta processing
//! - `stream_opened.rs` - Stream routing (blob vs sync)
//! - `blob_protocol.rs` - Blob protocol implementation
//! - This file - Simple event handlers (subscriptions, blobs, listening)

use crate::handlers::{state_delta, stream_opened};

use actix::{AsyncContext, Handler, WrapFuture};
use calimero_network_primitives::messages::NetworkEvent;
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_primitives::context::ContextId;
use tracing::{debug, error, info, warn};

use crate::NodeManager;

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
                    debug!(
                        %context_id,
                        %peer_id,
                        "Observed subscription to unknown context, ignoring.."
                    );
                    return;
                }

                info!("Peer '{}' subscribed to context '{}'", peer_id, context_id);
            }

            NetworkEvent::Unsubscribed { peer_id, topic } => {
                let Ok(context_id): Result<ContextId, _> = topic.as_str().parse() else {
                    return;
                };

                info!(
                    "Peer '{}' unsubscribed from context '{}'",
                    peer_id, context_id
                );
            }

            // BroadcastMessage handling - delegate to state_delta module
            NetworkEvent::Message { message, .. } => {
                let Some(source) = message.source else {
                    warn!(?message, "Received message without source");
                    return;
                };

                let message = match borsh::from_slice::<BroadcastMessage<'_>>(&message.data) {
                    Ok(message) => message,
                    Err(err) => {
                        debug!(?err, ?message, "Failed to deserialize message");
                        return;
                    }
                };

                #[expect(clippy::match_same_arms, reason = "clearer separation")]
                match message {
                    BroadcastMessage::StateDelta {
                        context_id,
                        author_id,
                        delta_id,
                        parent_ids,
                        root_hash,
                        artifact,
                        nonce,
                        events,
                    } => {
                        info!(
                            %context_id,
                            %author_id,
                            delta_id = ?delta_id,
                            parent_count = parent_ids.len(),
                            has_events = events.is_some(),
                            "Matched StateDelta message"
                        );

                        // Clone the components we need
                        let node_clients = self.clients.clone();
                        let node_state = self.state.clone();
                        let network_client = self.managers.sync.network_client.clone();
                        let sync_config_timeout = self.managers.sync.sync_config.timeout;

                        let _ignored = ctx.spawn(
                            async move {
                                if let Err(err) = state_delta::handle_state_delta(
                                    node_clients,
                                    node_state,
                                    network_client,
                                    sync_config_timeout,
                                    source,
                                    context_id,
                                    author_id,
                                    delta_id,
                                    parent_ids,
                                    root_hash,
                                    artifact.into_owned(),
                                    nonce,
                                    events.map(|e| e.into_owned()),
                                )
                                .await
                                {
                                    warn!(?err, "Failed to handle state delta");
                                }
                            }
                            .into_actor(self),
                        );
                    }
                    BroadcastMessage::HashHeartbeat {
                        context_id,
                        root_hash: their_root_hash,
                        dag_heads: their_dag_heads,
                    } => {
                        let context_client = self.clients.context.clone();

                        // Check for divergence
                        if let Ok(Some(our_context)) = context_client.get_context(&context_id) {
                            // Compare DAG heads
                            let our_heads_set: std::collections::HashSet<_> =
                                our_context.dag_heads.iter().collect();
                            let their_heads_set: std::collections::HashSet<_> =
                                their_dag_heads.iter().collect();

                            // If we have the same DAG heads but different root hashes, we diverged!
                            if our_heads_set == their_heads_set
                                && our_context.root_hash != their_root_hash
                            {
                                error!(
                                    %context_id,
                                    ?source,
                                    our_hash = ?our_context.root_hash,
                                    their_hash = ?their_root_hash,
                                    dag_heads = ?their_dag_heads,
                                    "DIVERGENCE DETECTED: Same DAG heads but different root hash!"
                                );

                                // Trigger sync to recover from divergence
                                // The periodic sync will eventually run state sync protocol
                                warn!(
                                    %context_id,
                                    ?source,
                                    their_heads = ?their_dag_heads,
                                    "Divergence detected - periodic sync will recover"
                                );
                            } else if our_context.root_hash != their_root_hash {
                                debug!(
                                    %context_id,
                                    ?source,
                                    our_heads_count = our_context.dag_heads.len(),
                                    their_heads_count = their_dag_heads.len(),
                                    "Different root hash (normal - different DAG heads)"
                                );
                            }
                        }
                    }
                    _ => {
                        warn!(?message, "Received unexpected broadcast message type (not StateDelta or HashHeartbeat)");
                    }
                }
            }

            // Stream routing - delegate to stream_opened module
            NetworkEvent::StreamOpened {
                peer_id,
                stream,
                protocol,
            } => {
                stream_opened::handle_stream_opened(self, ctx, peer_id, stream, protocol);
            }

            // Blob events - simple logging (applications can listen to these)
            NetworkEvent::BlobRequested {
                blob_id,
                context_id,
                requesting_peer,
            } => {
                debug!(
                    blob_id = %blob_id,
                    context_id = %context_id,
                    requesting_peer = %requesting_peer,
                    "Blob requested by peer"
                );
                // Applications can listen to this event for custom logic
            }

            NetworkEvent::BlobProvidersFound {
                blob_id,
                context_id,
                providers,
            } => {
                debug!(
                    blob_id = %blob_id,
                    context_id = ?context_id.as_ref().map(|id| id.to_string()),
                    providers_count = providers.len(),
                    "Blob providers found in DHT"
                );
                // Applications can listen to this event for custom logic
            }

            NetworkEvent::BlobDownloaded {
                blob_id,
                context_id,
                data,
                from_peer,
            } => {
                info!(
                    blob_id = %blob_id,
                    context_id = %context_id,
                    from_peer = %from_peer,
                    data_size = data.len(),
                    "Blob downloaded successfully from peer"
                );

                // Store the downloaded blob data to blobstore
                let blobstore = self.managers.blobstore.clone();
                let blob_data = data.clone();

                let _ignored = ctx.spawn(
                    async move {
                        // Convert data to async reader for blobstore.put()
                        let reader = &blob_data[..];

                        match blobstore.put(reader).await {
                            Ok((stored_blob_id, _hash, size)) => {
                                info!(
                                    requested_blob_id = %blob_id,
                                    stored_blob_id = %stored_blob_id,
                                    size = size,
                                    "Blob stored successfully"
                                );
                            }
                            Err(e) => {
                                error!(
                                    blob_id = %blob_id,
                                    error = %e,
                                    "Failed to store downloaded blob"
                                );
                            }
                        }
                    }
                    .into_actor(self),
                );
            }

            NetworkEvent::BlobDownloadFailed {
                blob_id,
                context_id,
                from_peer,
                error,
            } => {
                info!(
                    blob_id = %blob_id,
                    context_id = %context_id,
                    from_peer = %from_peer,
                    error = %error,
                    "Blob download failed"
                );
                // Applications can listen to this event for retry logic
            }
        }
    }
}
