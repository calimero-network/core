//! Blob Protocol - Public blob download (CALIMERO_BLOB_PROTOCOL)
//!
//! **Purpose**: Serve blobs over the public blob protocol (no context authentication).
//!
//! **Protocol**:
//! 1. Client sends JSON BlobRequest (blob_id, context_id)
//! 2. Server responds with JSON BlobResponse (found, size)
//! 3. If found, server streams binary chunks
//! 4. Server sends empty chunk to signal end
//!
//! **Note**: This is different from BlobShare (InitPayload::BlobShare) which uses
//! context authentication. This is for public blob downloads.

use std::time::Duration;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_network_primitives::stream::{Message as StreamMessage, Stream};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use futures_util::{SinkExt, StreamExt};
use libp2p::PeerId;
use serde::{Deserialize, Serialize};
use tokio::time::{sleep, timeout};
use tracing::{debug, info};

// Protocol constants
const BLOB_SERVE_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes total
const CHUNK_SEND_TIMEOUT: Duration = Duration::from_secs(30); // 30 seconds per chunk
const FLOW_CONTROL_DELAY: Duration = Duration::from_millis(10); // Delay between chunks

/// Blob request message
#[derive(Debug, Serialize, Deserialize)]
pub struct BlobRequest {
    pub blob_id: BlobId,
    pub context_id: ContextId,
}

/// Blob response message
#[derive(Debug, Serialize, Deserialize)]
pub struct BlobResponse {
    pub found: bool,
    pub size: Option<u64>,
}

/// Binary blob chunk
#[derive(Debug, BorshSerialize, BorshDeserialize)]
struct BlobChunk {
    data: Vec<u8>,
}

/// Handle blob protocol stream (public blob download)
///
/// This implements the CALIMERO_BLOB_PROTOCOL for public blob downloads.
/// Unlike BlobShare (InitPayload), this doesn't require context authentication.
///
/// # Arguments
/// * `node_client` - Client for blob operations
/// * `peer_id` - Peer requesting the blob
/// * `stream` - Stream for communication
///
/// # Protocol Flow
/// 1. Read JSON BlobRequest
/// 2. Look up blob in local storage
/// 3. Send JSON BlobResponse (found/not found + size)
/// 4. If found, stream binary chunks with flow control
/// 5. Send empty chunk to signal end
pub async fn handle_blob_protocol_stream(
    node_client: &NodeClient,
    peer_id: PeerId,
    mut stream: &mut Stream,
) -> eyre::Result<()> {
    info!(%peer_id, "Handling public blob protocol request");

    // Read blob request
    let first_message = match stream.next().await {
        Some(Ok(msg)) => msg,
        Some(Err(e)) => return Err(e.into()),
        None => return Ok(()),
    };

    let blob_request: BlobRequest = serde_json::from_slice(&first_message.data)?;

    // Serve blob with timeout
    timeout(BLOB_SERVE_TIMEOUT, async {
        // Try to get blob
        let blob_stream = node_client
            .get_blob(&blob_request.blob_id, None)
            .await?;

        let (response, blob_stream) = if let Some(blob_stream) = blob_stream {
            let blob_metadata = node_client
                .get_blob_info(blob_request.blob_id)
                .await?;
            let size = blob_metadata.map(|m| m.size).unwrap_or(0);

            info!(%peer_id, %blob_request.blob_id, size, "Blob found, will stream");
            (
                BlobResponse {
                    found: true,
                    size: Some(size),
                },
                Some(blob_stream),
            )
        } else {
            info!(%peer_id, %blob_request.blob_id, "Blob not found");
            (BlobResponse { found: false, size: None }, None)
        };

        // Send response
        timeout(
            CHUNK_SEND_TIMEOUT,
            stream.send(StreamMessage::new(serde_json::to_vec(&response)?)),
        )
        .await??;

        // Stream chunks if found
        if let Some(mut blob_stream) = blob_stream {
            let mut chunk_count = 0;
            let mut total_bytes = 0;

            while let Some(chunk_result) = blob_stream.next().await {
                let chunk = chunk_result?;
                chunk_count += 1;
                total_bytes += chunk.len();

                debug!(%peer_id, chunk_number = chunk_count, chunk_size = chunk.len(), "Sending chunk");

                // Send chunk
                timeout(
                    CHUNK_SEND_TIMEOUT,
                    stream.send(StreamMessage::new(borsh::to_vec(&BlobChunk {
                        data: chunk.to_vec(),
                    })?)),
                )
                .await??;

                // Flow control: small delay every 10 chunks
                if chunk_count % 10 == 0 {
                    sleep(FLOW_CONTROL_DELAY).await;
                }
            }

            // Send final empty chunk to signal end
            timeout(
                CHUNK_SEND_TIMEOUT,
                stream.send(StreamMessage::new(borsh::to_vec(&BlobChunk {
                    data: Vec::new(),
                })?)),
            )
            .await??;

            info!(%peer_id, chunks = chunk_count, bytes = total_bytes, "Blob streamed successfully");
        }

        Ok::<(), eyre::Report>(())
    })
    .await?
}

