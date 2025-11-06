//! Blob protocol stream handling for direct P2P transfers.

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
use tracing::{debug, info, warn};

const BLOB_SERVE_TIMEOUT: Duration = Duration::from_secs(300);
const CHUNK_SEND_TIMEOUT: Duration = Duration::from_secs(30);
const FLOW_CONTROL_DELAY: Duration = Duration::from_millis(10);

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct BlobRequest {
    pub blob_id: BlobId,
    pub context_id: ContextId,
}

#[derive(Debug, Serialize, Deserialize)]
struct BlobResponse {
    found: bool,
    size: Option<u64>,
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
struct BlobChunk {
    data: Vec<u8>,
}

pub(crate) async fn handle_blob_protocol_stream(
    node_client: NodeClient,
    peer_id: PeerId,
    mut stream: Box<Stream>,
) -> eyre::Result<()> {
    info!(%peer_id, "Starting blob protocol stream handler");

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

    let blob_request = serde_json::from_slice::<BlobRequest>(&first_message.data)
        .map_err(|e| eyre::eyre!("Failed to parse blob request: {}", e))?;

    handle_blob_request_stream(node_client, peer_id, blob_request, stream).await
}

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

    let serve_result = timeout(BLOB_SERVE_TIMEOUT, async {
        info!(%peer_id, blob_id = %blob_request.blob_id, "Attempting to get blob from local storage");
        let blob_stream = node_client
            .get_blob(&BlobId::from(blob_request.blob_id), None)
            .await?;

        let (response, blob_stream) = if let Some(blob_stream) = blob_stream {
            info!(%peer_id, "Blob found, will stream chunks");

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

        let response_data = serde_json::to_vec(&response)
            .map_err(|e| eyre::eyre!("Failed to serialize blob response: {}", e))?;

        timeout(
            CHUNK_SEND_TIMEOUT,
            stream.send(StreamMessage::new(response_data)),
        )
        .await
        .map_err(|_| eyre::eyre!("Timeout sending response"))?
        .map_err(|e| eyre::eyre!("Failed to send blob response: {}", e))?;

        if response.found {
            let mut blob_stream = blob_stream.expect("blob stream must exist when response.found is true");

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

                        timeout(
                            CHUNK_SEND_TIMEOUT,
                            stream.send(StreamMessage::new(chunk_data)),
                        )
                        .await
                        .map_err(|_| eyre::eyre!("Timeout sending chunk {}", chunk_count))?
                        .map_err(|e| eyre::eyre!("Failed to send blob chunk: {}", e))?;

                        if chunk_count % 10 == 0 {
                            sleep(FLOW_CONTROL_DELAY).await;
                        }
                    }
                    Err(e) => {
                        warn!(%peer_id, error = %e, "Failed to read blob chunk");
                        return Err(eyre::eyre!("Failed to read blob chunk: {}", e));
                    }
                }
            }

            let final_chunk = BlobChunk { data: Vec::new() };

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
                total_chunks = chunk_count + 1,
                total_bytes = total_bytes_sent,
                "Successfully streamed all blob chunks"
            );
        }

        debug!(%peer_id, "Blob request stream handled successfully");
        Ok(())
    })
    .await;

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
