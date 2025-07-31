use std::num::NonZeroUsize;
use std::time::Duration;

use actix::{AsyncContext, Handler, Message, WrapFuture};
use calimero_context_primitives::client::ContextClient;
use calimero_crypto::{Nonce, SharedKey};
use calimero_network_primitives::messages::NetworkEvent;
use calimero_network_primitives::stream::{Message as StreamMessage, Stream};
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use eyre::bail;
use libp2p::PeerId;
use owo_colors::OwoColorize;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

use crate::utils::choose_stream;
use crate::NodeManager;

// Timeout and flow control settings for blob serving
const BLOB_SERVE_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes total
const CHUNK_SEND_TIMEOUT: Duration = Duration::from_secs(30); // 30 seconds per chunk
const FLOW_CONTROL_DELAY: Duration = Duration::from_millis(10); // Small delay between chunks

#[derive(Debug, Serialize, Deserialize)]
struct BlobRequest {
    blob_id: [u8; 32],
    context_id: [u8; 32],
}

#[derive(Debug, Serialize, Deserialize)]
struct BlobResponse {
    found: bool,
    size: Option<u64>, // Total size if found
}

// Use binary format for efficient chunk transfer
#[derive(Debug)]
struct BlobChunk {
    data: Vec<u8>,
    is_final: bool,
}

impl BlobChunk {
    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.data.len() + 9); // 8 bytes for length + 1 byte for is_final
        bytes.extend_from_slice(&(self.data.len() as u64).to_le_bytes());
        bytes.push(if self.is_final { 1u8 } else { 0u8 });
        bytes.extend_from_slice(&self.data);
        bytes
    }
}

/// Handle blob requests that come over streams
async fn handle_blob_request_stream(
    node_client: NodeClient,
    peer_id: libp2p::PeerId,
    blob_request: BlobRequest,
    mut stream: Box<Stream>,
) -> eyre::Result<()> {
    debug!(
        %peer_id,
        blob_id = %hex::encode(blob_request.blob_id),
        context_id = %hex::encode(blob_request.context_id),
        "Processing blob request stream using binary chunk protocol"
    );

    // Wrap the entire blob serving in a timeout
    let serve_result = timeout(BLOB_SERVE_TIMEOUT, async {
        // Try to get the blob as a stream (handles chunked blobs efficiently)
        let blob_stream = node_client
            .get_blob(&BlobId::from(blob_request.blob_id), None)
            .await?;

        let response = if let Some(_blob_stream) = blob_stream {
            debug!(%peer_id, "Blob found, will stream chunks");

            // Get blob metadata to determine size
            let blob_metadata = node_client
                .get_blob_info(BlobId::from(blob_request.blob_id))
                .await?;

            let total_size = blob_metadata.map(|meta| meta.size).unwrap_or(0);

            BlobResponse {
                found: true,
                size: Some(total_size),
            }
        } else {
            debug!(%peer_id, "Blob not found");
            BlobResponse {
                found: false,
                size: None,
            }
        };

        // Send initial response with timeout
        let response_data = serde_json::to_vec(&response)
            .map_err(|e| eyre::eyre!("Failed to serialize blob response: {}", e))?;

        timeout(
            CHUNK_SEND_TIMEOUT,
            stream.send(StreamMessage::new(response_data)),
        )
        .await
        .map_err(|_| eyre::eyre!("Timeout sending response"))?
        .map_err(|e| eyre::eyre!("Failed to send blob response: {}", e))?;

        // If blob was found, stream the chunks
        if response.found {
            let mut blob_stream = node_client
                .get_blob(&BlobId::from(blob_request.blob_id), None)
                .await?
                .expect("Blob should exist since we just checked"); // Safe because we checked above

            debug!(%peer_id, "Starting to stream blob chunks");

            let mut chunk_count = 0;
            let mut total_bytes_sent = 0;

            while let Some(chunk_result) = blob_stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        chunk_count += 1;
                        total_bytes_sent += chunk.len();

                        debug!(
                            %peer_id,
                            chunk_number = chunk_count,
                            chunk_size = chunk.len(),
                            total_sent = total_bytes_sent,
                            "Sending blob chunk"
                        );

                        let blob_chunk = BlobChunk {
                            data: chunk.to_vec(),
                            is_final: false,
                        };

                        let chunk_data = blob_chunk.to_bytes();

                        debug!(
                            %peer_id,
                            chunk_number = chunk_count,
                            original_chunk_size = chunk.len(),
                            binary_message_size = chunk_data.len(),
                            is_final = blob_chunk.is_final,
                            "Sending binary chunk data"
                        );

                        // Send chunk with timeout
                        timeout(
                            CHUNK_SEND_TIMEOUT,
                            stream.send(StreamMessage::new(chunk_data)),
                        )
                        .await
                        .map_err(|_| eyre::eyre!("Timeout sending chunk {}", chunk_count))?
                        .map_err(|e| eyre::eyre!("Failed to send blob chunk: {}", e))?;

                        // Add small delay for flow control to prevent overwhelming receiver
                        if chunk_count % 10 == 0 {
                            // Every 10 chunks (~10MB), add a small pause
                            sleep(FLOW_CONTROL_DELAY).await;
                        }
                    }
                    Err(e) => {
                        warn!(%peer_id, error = %e, "Failed to read blob chunk");
                        return Err(eyre::eyre!("Failed to read blob chunk: {}", e));
                    }
                }
            }

            // Send final empty chunk to signal end of stream
            let final_chunk = BlobChunk {
                data: Vec::new(),
                is_final: true,
            };

            let final_chunk_data = final_chunk.to_bytes();

            timeout(
                CHUNK_SEND_TIMEOUT,
                stream.send(StreamMessage::new(final_chunk_data)),
            )
            .await
            .map_err(|_| eyre::eyre!("Timeout sending final chunk"))?
            .map_err(|e| eyre::eyre!("Failed to send final blob chunk: {}", e))?;

            debug!(
                %peer_id,
                total_chunks = chunk_count + 1, // +1 for final chunk
                total_bytes = total_bytes_sent,
                "Successfully streamed all blob chunks"
            );
        }

        debug!(%peer_id, "Blob request stream handled successfully");
        Ok(())
    })
    .await;

    // Handle timeout result
    match serve_result {
        Ok(result) => result,
        Err(_) => {
            warn!(
                %peer_id,
                blob_id = %hex::encode(blob_request.blob_id),
                timeout_secs = BLOB_SERVE_TIMEOUT.as_secs(),
                "Blob serving timed out"
            );
            Err(eyre::eyre!("Blob serving timed out"))
        }
    }
}

/// Handle streams that arrived on the blob protocol
async fn handle_blob_protocol_stream(
    node_client: NodeClient,
    peer_id: libp2p::PeerId,
    mut stream: Box<Stream>,
) -> eyre::Result<()> {
    // Read the first message which should be a blob request
    let first_message = match stream.next().await {
        Some(Ok(msg)) => msg,
        Some(Err(e)) => {
            debug!(%peer_id, error = %e, "Error reading blob request from stream");
            return Err(e.into());
        }
        None => {
            debug!(%peer_id, "Blob protocol stream closed immediately");
            return Ok(());
        }
    };

    // Parse as blob request
    let blob_request = serde_json::from_slice::<BlobRequest>(&first_message.data)
        .map_err(|e| eyre::eyre!("Failed to parse blob request: {}", e))?;

    // Delegate to the existing handler
    handle_blob_request_stream(node_client, peer_id, blob_request, stream).await
}

impl Handler<NetworkEvent> for NodeManager {
    type Result = <NetworkEvent as Message>::Result;

    fn handle(&mut self, msg: NetworkEvent, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            NetworkEvent::ListeningOn { address, .. } => info!("Listening on: {}", address),
            NetworkEvent::Subscribed { peer_id, topic } => {
                let Ok(context_id): Result<ContextId, _> = topic.as_str().parse() else {
                    return;
                };

                if !self
                    .context_client
                    .has_context(&context_id)
                    .unwrap_or_default()
                {
                    debug!(
                        %context_id,
                        %their_peer_id,
                        "Observed subscription to unknown context, ignoring.."
                    );

                    return;
                }

                info!(
                    "Peer '{}' subscribed to context '{}'",
                    their_peer_id.cyan(),
                    context_id.cyan()
                );
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
            NetworkEvent::Message { message, .. } => {
                let Some(source) = message.source else {
                    warn!(?message, "Received message without source");
                    return;
                };

                let message = match borsh::from_slice(&message.data) {
                    Ok(message) => message,
                    Err(err) => {
                        debug!(?err, ?message, "Failed to deserialize message");
                        return;
                    }
                };

                match message {
                    BroadcastMessage::StateDelta {
                        context_id,
                        author_id,
                        root_hash,
                        artifact,
                        height,
                        nonce,
                    } => {
                        let node_client = self.node_client.clone();
                        let context_client = self.context_client.clone();

                        let _ignored = ctx.spawn(
                            async move {
                                if let Err(err) = handle_state_delta(
                                    node_client,
                                    context_client,
                                    source,
                                    context_id,
                                    author_id,
                                    root_hash,
                                    artifact.into_owned(),
                                    height,
                                    nonce,
                                )
                                .await
                                {
                                    warn!(?err, "Failed to handle state delta");
                                }
                            }
                            .into_actor(self),
                        );
                    }
                    _ => {
                        debug!(?message, "Received unexpected message");
                    }
                }
            }
            NetworkEvent::StreamOpened {
                peer_id,
                stream,
                protocol,
            } => {
                // Route streams based on protocol
                if protocol == calimero_network_primitives::stream::CALIMERO_BLOB_PROTOCOL {
                    debug!(%peer_id, "Handling blob protocol stream");
                    let node_client = self.node_client.clone();
                    let _ignored = ctx.spawn(
                        async move {
                            if let Err(err) = handle_blob_protocol_stream(node_client, peer_id, stream).await {
                                debug!(%peer_id, error = %err, "Failed to handle blob protocol stream");
                            }
                        }
                        .into_actor(self),
                    );
                } else {
                    debug!(%peer_id, "Handling sync protocol stream");
                    let sync_manager = self.sync_manager.clone();
                    let _ignored = ctx.spawn(
                        async move {
                            sync_manager.handle_opened_stream(stream).await;
                        }
                        .into_actor(self),
                    );
                }
            }
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
                // For now, just log the request. Applications can listen to this event
                // to implement custom logic when blobs are requested.
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
                // For now, just log the discovery. Applications can listen to this event
                // to implement custom logic when providers are found.
            }
            NetworkEvent::BlobDownloaded {
                blob_id,
                context_id,
                data,
                from_peer,
            } => {
                debug!(
                    blob_id = %blob_id,
                    context_id = %context_id,
                    from_peer = %from_peer,
                    data_size = data.len(),
                    "Blob downloaded successfully from peer"
                );
                // For now, just log the success. Applications can listen to this event
                // to implement custom logic when blobs are downloaded.
            }
            NetworkEvent::BlobDownloadFailed {
                blob_id,
                context_id,
                from_peer,
                error,
            } => {
                debug!(
                    blob_id = %blob_id,
                    context_id = %context_id,
                    from_peer = %from_peer,
                    error = %error,
                    "Blob download failed"
                );
                // For now, just log the failure. Applications can listen to this event
                // to implement retry logic or fallback behavior.
            }
        }
    }
}

async fn handle_state_delta(
    node_client: NodeClient,
    context_client: ContextClient,
    source: PeerId,
    sync_manager: SyncManager,
    context_id: ContextId,
    author_id: PublicKey,
    root_hash: Hash,
    artifact: Vec<u8>,
    height: NonZeroUsize,
    nonce: Nonce,
) -> eyre::Result<()> {
    let Some(context) = context_client.get_context(&context_id)? else {
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

    if let Some(known_height) = context_client.get_delta_height(&context_id, &author_id)? {
        if known_height >= height || height.get() - known_height.get() > 1 {
            debug!(%author_id, %context_id, "Received state delta much further ahead than known height, syncing..");

            node_client.sync(Some(&context_id), Some(&source)).await?;
            return Ok(());
        }
    }

    let Some(sender_key) = context_client
        .get_identity(&context_id, &author_id)?
        .and_then(|i| i.sender_key)
    else {
        debug!(%author_id, %context_id, "Missing sender key, initiating sync");

        node_client.sync(Some(&context_id), Some(&source)).await?;
        return Ok(());
    };

    let shared_key = SharedKey::from_sk(&sender_key);

    let Some(artifact) = shared_key.decrypt(artifact, nonce) else {
        debug!(%author_id, %context_id, "State delta decryption failed, initiating sync");

        node_client.sync(Some(&context_id), Some(&source)).await?;
        return Ok(());
    };

    let identities = context_client.context_members(&context_id, Some(true));

    let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
        .await
        .transpose()?
    else {
        bail!("no owned identities found for context: {}", context_id);
    };

    context_client.put_state_delta(&context_id, &author_id, &height, &artifact)?;

    let outcome = context_client
        .execute(
            &context_id,
            &our_identity,
            "__calimero_sync_next".to_owned(),
            artifact,
            vec![],
            None,
        )
        .await?;

    context_client.set_delta_height(&context_id, &author_id, height)?;

    if outcome.root_hash != root_hash {
        debug!(
            %context_id,
            %author_id,
            expected_root_hash = %root_hash,
            current_root_hash = %outcome.root_hash,
            "State delta application led to root hash mismatch, ignoring for now"
        );

        //     debug!(
        //         %context_id,
        //         %author_id,
        //         expected_root_hash = %root_hash,
        //         current_root_hash = %outcome.root_hash,
        //         "State delta application led to root hash mismatch, initiating sync"
        //     );

        //     let _ignored = sync_manager.initiate_sync(context_id, source).await;
    }

    Ok(())
}
