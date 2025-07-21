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
use futures_util::{SinkExt, StreamExt};
use hex;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::sync::SyncManager;
use crate::utils::choose_stream;
use crate::NodeManager;

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

#[derive(Debug, Serialize, Deserialize)]
struct BlobChunk {
    data: Vec<u8>,
    is_final: bool, // True for the last chunk
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
        "Processing blob request stream"
    );

    // Try to get the blob as a stream (handles chunked blobs efficiently)
    let blob_stream = node_client.get_blob(&BlobId::from(blob_request.blob_id))?;

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

    // Send initial response
    let response_data = serde_json::to_vec(&response)
        .map_err(|e| eyre::eyre!("Failed to serialize blob response: {}", e))?;

    stream
        .send(StreamMessage::new(response_data))
        .await
        .map_err(|e| eyre::eyre!("Failed to send blob response: {}", e))?;

    // If blob was found, stream the chunks
    if response.found {
        let mut blob_stream = node_client
            .get_blob(&BlobId::from(blob_request.blob_id))?
            .expect("Blob should exist since we just checked"); // Safe because we checked above

        debug!(%peer_id, "Starting to stream blob chunks");

        while let Some(chunk_result) = blob_stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    let blob_chunk = BlobChunk {
                        data: chunk.to_vec(),
                        is_final: false,
                    };

                    let chunk_data = serde_json::to_vec(&blob_chunk)
                        .map_err(|e| eyre::eyre!("Failed to serialize blob chunk: {}", e))?;

                    stream
                        .send(StreamMessage::new(chunk_data))
                        .await
                        .map_err(|e| eyre::eyre!("Failed to send blob chunk: {}", e))?;
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

        let final_chunk_data = serde_json::to_vec(&final_chunk)
            .map_err(|e| eyre::eyre!("Failed to serialize final blob chunk: {}", e))?;

        stream
            .send(StreamMessage::new(final_chunk_data))
            .await
            .map_err(|e| eyre::eyre!("Failed to send final blob chunk: {}", e))?;

        debug!(%peer_id, "Successfully streamed all blob chunks");
    }

    debug!(%peer_id, "Blob request stream handled successfully");
    Ok(())
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
                        nonce,
                    } => {
                        let context_client = self.context_client.clone();
                        let sync_manager = self.sync_manager.clone();

                        let _ignored = ctx.spawn(
                            async move {
                                if let Err(err) = handle_state_delta(
                                    context_client,
                                    sync_manager,
                                    source,
                                    context_id,
                                    author_id,
                                    root_hash,
                                    artifact.into_owned(),
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
    context_client: ContextClient,
    sync_manager: SyncManager,
    source: libp2p::PeerId,
    context_id: ContextId,
    author_id: PublicKey,
    root_hash: Hash,
    artifact: Vec<u8>,
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

    let Some(sender_key) = context_client
        .get_identity(&context_id, &author_id)?
        .and_then(|i| i.sender_key)
    else {
        debug!(%author_id, %context_id, "Missing sender key, initiating sync");

        return sync_manager.initiate_sync(context_id, source).await;
    };

    let shared_key = SharedKey::from_sk(&sender_key);

    let Some(artifact) = shared_key.decrypt(artifact, nonce) else {
        debug!(%author_id, %context_id, "State delta decryption failed, initiating sync");

        return sync_manager.initiate_sync(context_id, source).await;
    };

    let identities = context_client.context_members(&context_id, Some(true));

    let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
        .await
        .transpose()?
    else {
        bail!("no owned identities found for context: {}", context.id);
    };

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

    if outcome.root_hash != root_hash {
        return sync_manager.initiate_sync(context_id, source).await;
    }

    Ok(())
}
