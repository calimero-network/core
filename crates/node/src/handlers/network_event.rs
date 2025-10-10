//! Network Event Handler
//!
//! This module handles incoming network events from other nodes in the Calimero network.
//! It processes various types of network messages including state deltas, blob requests,
//! and peer subscription events.
//!
//! Key responsibilities:
//! - Processing state delta broadcasts and applying them to local contexts
//! - Handling blob requests and serving blob data over streams
//! - Managing peer subscriptions and unsubscriptions to contexts
//! - Processing events for automatic callbacks when state deltas are received
//!
//! The main entry point is the `Handler<NetworkEvent>` implementation for `NodeManager`.

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
use calimero_primitives::events::{
    ContextEvent, ContextEventPayload, ExecutionEvent, NodeEvent, StateMutationPayload,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt};
use futures_util::{SinkExt, StreamExt};
use libp2p::PeerId;
use serde::{Deserialize, Serialize};
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

use crate::utils::choose_stream;
use crate::NodeManager;

// Timeout and flow control settings for blob serving
const BLOB_SERVE_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes total
const CHUNK_SEND_TIMEOUT: Duration = Duration::from_secs(30); // 30 seconds per chunk
const FLOW_CONTROL_DELAY: Duration = Duration::from_millis(10); // Small delay between chunks

/// Request structure for blob retrieval over network streams
#[derive(Debug, Serialize, Deserialize)]
struct BlobRequest {
    /// The unique identifier of the blob to retrieve
    blob_id: BlobId,
    /// The context ID that owns this blob
    context_id: ContextId,
}

/// Response structure for blob requests
#[derive(Debug, Serialize, Deserialize)]
struct BlobResponse {
    /// Whether the blob was found in local storage
    found: bool,
    /// Total size of the blob if found (for progress tracking)
    size: Option<u64>,
}

// Use binary format for efficient chunk transfer
use borsh::{BorshDeserialize, BorshSerialize};

/// Binary chunk structure for streaming blob data
/// This allows efficient transfer of large blobs by breaking them into manageable chunks
#[derive(Debug, BorshSerialize, BorshDeserialize)]
struct BlobChunk {
    /// Raw binary data for this chunk
    data: Vec<u8>,
}

/// Handle blob requests that come over network streams
///
/// This function processes blob requests from other nodes and serves the requested blob data
/// in chunks over a network stream. It implements a robust streaming protocol with:
/// - Timeout handling for long-running transfers
/// - Flow control to prevent overwhelming the network
/// - Binary serialization for efficient data transfer
/// - Error handling and recovery
///
/// # Arguments
/// * `node_client` - Client for accessing local blob storage
/// * `peer_id` - ID of the peer requesting the blob
/// * `blob_request` - Details of the blob being requested
/// * `stream` - Network stream for sending data to the requesting peer
async fn handle_blob_request_stream(
    node_client: NodeClient,
    peer_id: PeerId,
    blob_request: BlobRequest,
    mut stream: Box<Stream>,
) -> eyre::Result<()> {
    info!(
        %peer_id,
        blob_id = blob_request.blob_id.as_str(),
        context_id = blob_request.context_id.as_str(),
        "Processing blob request stream using binary chunk protocol"
    );

    // Wrap the entire blob serving in a timeout
    let serve_result = timeout(BLOB_SERVE_TIMEOUT, async {
        // Try to get the blob as a stream (handles chunked blobs efficiently)
        info!(%peer_id, blob_id = %blob_request.blob_id, "Attempting to get blob from local storage");
        let blob_stream = node_client
            .get_blob(&BlobId::from(blob_request.blob_id), None)
            .await?;

        let (response, blob_stream) = if let Some(blob_stream) = blob_stream {
            info!(%peer_id, "Blob found, will stream chunks");

            // Get blob metadata to determine size
            let blob_metadata = node_client
                .get_blob_info(BlobId::from(blob_request.blob_id))
                .await?;

            let total_size = blob_metadata.map(|meta| meta.size).unwrap_or(0);

            let response = BlobResponse {
                found: true,
                size: Some(total_size),
            };

            (response, Some(blob_stream))
        } else {
            info!(%peer_id, "Blob not found");
            let response = BlobResponse {
                found: false,
                size: None,
            };
            (response, None)
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
            let mut blob_stream = blob_stream.expect("Blob stream should exist since response.found is true");

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
                        };

                        let chunk_data = borsh::to_vec(&blob_chunk)
                            .map_err(|e| eyre::eyre!("Failed to serialize blob chunk: {}", e))?;

                        debug!(
                            %peer_id,
                            chunk_number = chunk_count,
                            original_chunk_size = chunk.len(),
                            binary_message_size = chunk_data.len(),
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
            };

            let final_chunk_data = borsh::to_vec(&final_chunk)
                .map_err(|e| eyre::eyre!("Failed to serialize final chunk: {}", e))?;

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
                blob_id = blob_request.blob_id.as_str(),
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
    peer_id: PeerId,
    mut stream: Box<Stream>,
) -> eyre::Result<()> {
    info!(%peer_id, "Starting blob protocol stream handler");

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

/// Network Event Handler Implementation
///
/// This handler processes various types of network events from other nodes in the Calimero network.
/// It's the main entry point for handling incoming network messages and coordinating responses.
impl Handler<NetworkEvent> for NodeManager {
    type Result = <NetworkEvent as Message>::Result;

    /// Handle incoming network events
    ///
    /// This method dispatches different types of network events to appropriate handlers:
    /// - `ListeningOn`: Logs when the node starts listening on a network address
    /// - `Subscribed`/`Unsubscribed`: Manages peer subscriptions to contexts
    /// - `Message`: Processes broadcast messages (primarily state deltas)
    /// - `StreamOpened`: Handles blob request streams
    fn handle(&mut self, msg: NetworkEvent, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            NetworkEvent::ListeningOn { address, .. } => info!("Listening on: {}", address),
            // Handle peer subscriptions to contexts
            // When a peer subscribes to a context, they will receive state delta broadcasts
            // for that context. We only acknowledge subscriptions to contexts we know about.
            NetworkEvent::Subscribed { peer_id, topic } => {
                let Ok(context_id): Result<ContextId, _> = topic.as_str().parse() else {
                    return;
                };

                // Only acknowledge subscriptions to contexts we have locally
                if !self
                    .context_client
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
            // Handle peer unsubscriptions from contexts
            // This is mainly for logging purposes as the network layer handles the cleanup
            NetworkEvent::Unsubscribed { peer_id, topic } => {
                let Ok(context_id): Result<ContextId, _> = topic.as_str().parse() else {
                    return;
                };

                info!(
                    "Peer '{}' unsubscribed from context '{}'",
                    peer_id, context_id
                );
            }
            // Handle incoming network messages
            // Messages are deserialized and dispatched to appropriate handlers
            NetworkEvent::Message { message, .. } => {
                let Some(source) = message.source else {
                    warn!(?message, "Received message without source");
                    return;
                };

                // Deserialize the message using Borsh for efficient binary serialization
                let message = match borsh::from_slice(&message.data) {
                    Ok(message) => message,
                    Err(err) => {
                        debug!(?err, ?message, "Failed to deserialize message");
                        return;
                    }
                };

                // Dispatch the message to the appropriate handler
                match message {
                    // Handle state delta broadcasts - the most common message type
                    // State deltas contain state changes and events from other nodes
                    BroadcastMessage::StateDelta {
                        context_id,
                        author_id,
                        root_hash,
                        artifact,
                        height,
                        nonce,
                        events,
                    } => {
                        let node_client = self.node_client.clone();
                        let context_client = self.context_client.clone();

                        // Process the state delta asynchronously to avoid blocking the main handler
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
                    _ => {
                        debug!(?message, "Received unexpected message");
                    }
                }
            }
            // Handle incoming network streams
            // Streams are used for efficient data transfer, particularly for large blobs
            NetworkEvent::StreamOpened {
                peer_id,
                stream,
                protocol,
            } => {
                // Route streams based on protocol type
                if protocol == calimero_network_primitives::stream::CALIMERO_BLOB_PROTOCOL {
                    // Handle blob request streams for serving large binary data
                    info!(%peer_id, "Handling blob protocol stream - STREAM OPENED");
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
                    // Handle synchronization streams for state delta synchronization
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
            // Handle blob request events
            // These events are generated when other nodes request blobs from this node
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
            // Handle blob provider discovery events
            // These events are generated when the DHT finds providers for requested blobs
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
            // Handle blob download completion events
            // These events are generated when blobs are successfully downloaded from other nodes
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
                    "Blob downloaded successfully from peer, storing to blobstore"
                );

                // Store the downloaded blob data to the local blobstore
                // This ensures the blob is available for future requests
                let blobstore = self.blobstore.clone();
                let blob_data = data.clone();

                let _ = ctx.spawn(
                    async move {
                        // Convert data to async reader for blobstore.put()
                        let reader = &blob_data[..];

                        match blobstore.put(reader).await {
                            Ok((stored_blob_id, _hash, size)) => {
                                debug!(
                                    requested_blob_id = %blob_id,
                                    stored_blob_id = %stored_blob_id,
                                    size = size,
                                    "Successfully stored downloaded blob"
                                );
                            }
                            Err(e) => {
                                warn!(
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

/// Handle incoming state delta broadcasts from other nodes
///
/// This function processes state delta messages that contain state changes and events
/// from other nodes. It performs several critical operations:
///
/// 1. **State Synchronization**: Applies the state delta to the local context using `__calimero_sync_next`
/// 2. **Event Processing**: Processes any events contained in the delta for automatic callbacks
/// 3. **Height Management**: Tracks and validates the height progression to prevent out-of-order updates
/// 4. **Conflict Resolution**: Handles cases where the delta is ahead of the current state
///
/// # Arguments
/// * `node_client` - Client for node operations
/// * `context_client` - Client for context operations
/// * `source` - Peer ID of the node that sent the delta
/// * `context_id` - ID of the context being updated
/// * `author_id` - Public key of the node that authored the changes
/// * `root_hash` - New root hash after applying the delta
/// * `artifact` - State change artifact data
/// * `height` - Height of this delta in the author's sequence
/// * `nonce` - Nonce for this delta
/// * `events` - Optional events data for callback processing
async fn handle_state_delta(
    node_client: NodeClient,
    context_client: ContextClient,
    source: PeerId,
    context_id: ContextId,
    author_id: PublicKey,
    root_hash: Hash,
    artifact: Vec<u8>,
    height: NonZeroUsize,
    nonce: Nonce,
    events: Option<Vec<u8>>,
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
            // Note: when falling back to sync, any bundled events in this broadcast
            // are intentionally skipped to avoid double processing.
            debug!(
                %context_id,
                %author_id,
                has_bundled_events = events.as_ref().map(|e| !e.is_empty()).unwrap_or(false),
                "Skipping bundled events due to sync fallback (height gap)"
            );

            node_client.sync(Some(&context_id), Some(&source)).await?;
            return Ok(());
        }
    }

    let Some(sender_key) = context_client
        .get_identity(&context_id, &author_id)?
        .and_then(|i| i.sender_key)
    else {
        debug!(%author_id, %context_id, "Missing sender key, initiating sync");
        // Note: when falling back to sync, any bundled events in this broadcast
        // are intentionally skipped to avoid double processing.
        debug!(
            %context_id,
            %author_id,
            has_bundled_events = events.as_ref().map(|e| !e.is_empty()).unwrap_or(false),
            "Skipping bundled events due to sync fallback (missing sender key)"
        );

        node_client.sync(Some(&context_id), Some(&source)).await?;
        return Ok(());
    };

    let shared_key = SharedKey::from_sk(&sender_key);

    let Some(artifact) = shared_key.decrypt(artifact, nonce) else {
        debug!(%author_id, %context_id, "State delta decryption failed, initiating sync");
        // Note: when falling back to sync, any bundled events in this broadcast
        // are intentionally skipped to avoid double processing.
        debug!(
            %context_id,
            %author_id,
            has_bundled_events = events.as_ref().map(|e| !e.is_empty()).unwrap_or(false),
            "Skipping bundled events due to sync fallback (decrypt failure)"
        );

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

    // Store the state delta for future reference
    context_client.put_state_delta(&context_id, &author_id, &height, &artifact)?;

    // Apply the state delta to the local context using the special sync method
    // This method only applies state changes and does not emit new events
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

    // Update the tracked height for this author to prevent out-of-order processing
    context_client.set_delta_height(&context_id, &author_id, height)?;

    if outcome.root_hash != root_hash {
        debug!(
            %context_id,
            %author_id,
            expected_root_hash = %root_hash,
            current_root_hash = %outcome.root_hash,
            "State delta application led to root hash mismatch, ignoring for now"
        );
    }

    // Process execution events if they were included in the state delta
    // Events are bundled with state deltas to ensure they're processed atomically
    // with the state changes that triggered them
    debug!(
        %context_id,
        %author_id,
        has_events = events.as_ref().map(|e| !e.is_empty()).unwrap_or(false),
        "Received StateDelta; checking for bundled events"
    );
    if let Some(events_data) = events {
        debug!(%context_id, raw_events_len = events_data.len(), "Raw events bytes received");
        let events_payload: Vec<ExecutionEvent> =
            serde_json::from_slice(&events_data).unwrap_or_else(|_| Vec::new());

        // Only re-emit if there are actual events
        if !events_payload.is_empty() {
            // Re-emit events to WebSocket clients as StateMutation events
            // This allows clients to receive real-time notifications about state changes
            debug!(%context_id, events_count = events_payload.len(), "Re-emitting events to WS as StateMutation");
            node_client.send_event(NodeEvent::Context(ContextEvent {
                context_id,
                payload: ContextEventPayload::StateMutation(
                    StateMutationPayload::with_root_and_events(root_hash, events_payload.clone()),
                ),
            }))?;

            // Process events for automatic callbacks
            // This is the correct place to handle event callbacks because:
            // 1. We're processing events received from other nodes via state delta broadcasts
            // 2. Callbacks should be instant and happen immediately when events are received
            // 3. This separates event processing from state delta synchronization logic
            // 4. It prevents double processing that would occur if callbacks were handled in delta.rs
            debug!(%context_id, "Processing events for automatic callbacks");
            for event in events_payload {
                debug!(
                    %context_id,
                    event_kind = %event.kind,
                    event_data_len = event.data.len(),
                    "Processing event for automatic callback"
                );

                // Call the application's event processing method
                // Encode arguments with Borsh to match WASM ABI (event_kind: String, event_data: Vec<u8>)
                let combined_payload = borsh::to_vec(&(event.kind.clone(), event.data.clone()))
                    .unwrap_or_default();

                // Execute the callback and commit the state changes
                match context_client
                    .execute(
                        &context_id,
                        &our_identity,
                        "process_remote_events".to_owned(),
                        combined_payload,
                        vec![], // No aliases needed
                        None,
                    )
                    .await
                {
                    Ok(callback_outcome) => {
                        debug!(
                            %context_id,
                            event_kind = %event.kind,
                            "Successfully processed event for automatic callback"
                        );
                        
                        // Commit the callback state changes by creating a new state delta
                        if !callback_outcome.artifact.is_empty() {
                            debug!(
                                %context_id,
                                artifact_len = callback_outcome.artifact.len(),
                                "Committing callback state changes"
                            );
                            
                            // Create a new state delta for the callback changes
                            let callback_height = height.get() + 1;
                            let callback_height = NonZeroUsize::new(callback_height)
                                .ok_or_eyre("callback height overflow")?;
                            
                            // Store the callback state delta
                            context_client.put_state_delta(
                                &context_id,
                                &our_identity,
                                &callback_height,
                                &callback_outcome.artifact,
                            )?;
                            
                            // Apply the callback state delta
                            let final_outcome = context_client
                                .execute(
                                    &context_id,
                                    &our_identity,
                                    "__calimero_sync_next".to_owned(),
                                    callback_outcome.artifact,
                                    vec![],
                                    None,
                                )
                                .await?;
                            
                            // Update the delta height
                            context_client.set_delta_height(&context_id, &our_identity, callback_height)?;
                            
                            debug!(
                                %context_id,
                                callback_height = callback_height.get(),
                                final_root_hash = %final_outcome.root_hash,
                                "Callback state changes committed"
                            );
                        }
                    }
                    Err(err) => {
                        debug!(
                            %context_id,
                            error = %err,
                            "Failed to process event for automatic callback"
                        );
                    }
                }
            }
        } else {
            debug!(%context_id, "No events after deserialization; skipping WS emit");
        }
    }

    Ok(())
}
