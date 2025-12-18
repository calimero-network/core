//! Blob protocol stream handling
//!
//! **SRP**: This module handles the blob protocol for P2P blob transfer
//! Implements chunked blob streaming with flow control and timeouts

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use calimero_context_primitives::client::ContextClient;
use calimero_network_primitives::{
    blob_types::{BlobAuthPayload, BlobChunk, BlobRequest, BlobResponse},
    stream::{Message as StreamMessage, Stream},
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::blobs::BlobId;
use futures_util::{SinkExt, StreamExt};
use libp2p::PeerId;
use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};

// Timeout and flow control settings for blob serving
const BLOB_SERVE_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes total
const CHUNK_SEND_TIMEOUT: Duration = Duration::from_secs(30); // 30 seconds per chunk
const FLOW_CONTROL_DELAY: Duration = Duration::from_millis(10); // Small delay between chunks

// Replay protection window (30 seconds past, 10 seconds future)
const MAX_REQUEST_AGE_SECS: u64 = 30;
const MAX_REQUEST_FUTURE_AGE_SECS: u64 = 10;

/// Handles streams that arrived on the blob protocol
///
/// Reads the first message as a BlobRequest, then delegates to the chunked handler.
pub async fn handle_blob_protocol_stream(
    node_client: NodeClient,
    context_client: ContextClient,
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

    if !is_blob_access_authorized(&context_client, &blob_request).await? {
        let response = BlobResponse {
            found: false,
            size: None,
        };
        let response_data = serde_json::to_vec(&response)?;

        timeout(
            CHUNK_SEND_TIMEOUT,
            stream.send(StreamMessage::new(response_data)),
        )
        .await
        .map_err(|_| eyre::eyre!("Timeout sending auth rejection"))??;

        return Ok(());
    }

    // Delegate to the chunked handler
    handle_blob_request_stream(node_client, peer_id, blob_request, stream).await
}

/// Handles blob requests that come over streams
///
/// This implements the chunked blob transfer protocol:
/// 1. Send BlobResponse (found/not found + size)
/// 2. If found, stream blob chunks
/// 3. Send empty chunk to signal end
///
/// Features:
/// - Flow control (delay every 10 chunks)
/// - Timeouts (5 min total, 30 sec per chunk)
/// - Binary chunk encoding for efficiency
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

/// Helper function to check if the blob access is authorized.
///
////// Helper function to authorize blob access.
///
/// Implements the security policy:
/// 1. Public blobs (App Bundles) are accessible to everyone (bootstrapping).
/// 2. Private blobs require a valid signature from a Context Member.
///
/// # Returns
/// * `Ok(true)` - if access is granted.
/// * `Ok(false)` - if access is denied.
/// * `Err` - only on internal system failures (e.g. DB errors).
async fn is_blob_access_authorized(
    context_client: &ContextClient,
    request: &BlobRequest,
) -> eyre::Result<bool> {
    // Fetch Context Config
    // If we don't have the context config, we can't verify anything. Deny access.
    let context_config = match context_client.context_config(&request.context_id) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => {
            warn!(context_id=%request.context_id, "Context config not found locally. Denying blob access.");
            return Ok(false);
        }
        Err(e) => return Err(e),
    };

    // Check if the Blob is Public (The Application Bundle)
    // New nodes need this to join, so they cannot sign yet.
    // We identify if the requested blob is the app bundle using the authoritative config.
    let external_client = context_client.external_client(&request.context_id, &context_config)?;
    let app_config = external_client.config().application().await;

    if let Ok(app) = app_config {
        let requested_blob = BlobId::from(request.blob_id);
        // Allow if it matches the bytecode or compiled artifact
        if requested_blob == app.blob.bytecode || requested_blob == app.blob.compiled {
            debug!(blob_id=%request.blob_id, "Access granted: Blob is public Application Bundle");
            return Ok(true);
        }
    } else {
        warn!("Failed to fetch application config to verify public blob.");
    }

    let auth = match &request.auth {
        Some(auth_struct) => auth_struct,
        None => return Ok(false),
    };

    // Replay Protection
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    if auth.timestamp < now.saturating_sub(MAX_REQUEST_AGE_SECS)
        || auth.timestamp > now.saturating_add(MAX_REQUEST_FUTURE_AGE_SECS)
    {
        return Ok(false);
    }

    // Reconstruct the Envelope Payload for Verification
    let payload = BlobAuthPayload {
        blob_id: *request.blob_id,
        context_id: *request.context_id,
        timestamp: auth.timestamp,
    };

    let message = borsh::to_vec(&payload)?;

    // Verify Signature
    if auth
        .public_key
        .verify_raw_signature(&message, &auth.signature)
        .is_err()
    {
        error!(blob_id=%request.blob_id, "The blob request had an auth header, but the signature is incorrect.");
        return Ok(false);
    }

    // Verify Context Membership
    let is_member = context_client.has_member(&request.context_id, &auth.public_key)?;
    if !is_member {
        error!(
            blob_id=%request.blob_id,
            %request.context_id,
            %auth.public_key,
            "The blob request had an auth header, but the identity is not a member of the context."
        );
    }

    Ok(is_member)
}
