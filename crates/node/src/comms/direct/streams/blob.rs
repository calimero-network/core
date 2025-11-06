use std::time::Duration;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_context_primitives::client::ContextClient;
use calimero_network_primitives::stream::{Message as StreamMessage, Stream};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::identity::PublicKey;
use eyre::{Result, WrapErr};
use futures_util::{SinkExt, StreamExt};
use libp2p::PeerId;
use serde::{Deserialize, Serialize};
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

use crate::comms::direct::blob_share;

const BLOB_SERVE_TIMEOUT: Duration = Duration::from_secs(300);
const CHUNK_SEND_TIMEOUT: Duration = Duration::from_secs(30);
const FLOW_CONTROL_DELAY: Duration = Duration::from_millis(10);

#[derive(Clone, Debug)]
pub(crate) struct BlobResponder {
    node_client: NodeClient,
    context_client: ContextClient,
}

impl BlobResponder {
    pub(crate) fn new(node_client: NodeClient, context_client: ContextClient) -> Self {
        Self {
            node_client,
            context_client,
        }
    }

    pub(crate) async fn handle_stream(
        &self,
        peer_id: PeerId,
        mut stream: Box<Stream>,
    ) -> Result<()> {
        info!(%peer_id, "Starting blob protocol responder");

        let Some(first_message) = stream
            .next()
            .await
            .transpose()
            .wrap_err("reading blob request")?
        else {
            debug!(%peer_id, "Blob protocol stream closed immediately");
            return Ok(());
        };

        let blob_request: BlobRequest =
            serde_json::from_slice(&first_message.data).wrap_err("failed to parse blob request")?;

        self.handle_blob_request(peer_id, blob_request, stream)
            .await
    }

    async fn handle_blob_request(
        &self,
        peer_id: PeerId,
        blob_request: BlobRequest,
        mut stream: Box<Stream>,
    ) -> Result<()> {
        info!(
            %peer_id,
            blob_id = blob_request.blob_id.as_str(),
            context_id = blob_request.context_id.as_str(),
            "Processing blob request stream using binary chunk protocol"
        );

        let serve_result = timeout(BLOB_SERVE_TIMEOUT, async {
            let blob_id = BlobId::from(blob_request.blob_id);

            info!(%peer_id, %blob_id, "Attempting to get blob from local storage");
            let blob_stream = self.node_client.get_blob(&blob_id, None).await?;

            let (response, blob_stream) = if let Some(blob_stream) = blob_stream {
                info!(%peer_id, "Blob found, will stream chunks");

                let blob_metadata = self.node_client.get_blob_info(blob_id.clone()).await?;
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

            let response_data =
                serde_json::to_vec(&response).wrap_err("serialize blob response")?;

            timeout(
                CHUNK_SEND_TIMEOUT,
                stream.send(StreamMessage::new(response_data)),
            )
            .await
            .map_err(|_| eyre::eyre!("Timeout sending blob response"))??;

            if response.found {
                let mut blob_stream =
                    blob_stream.expect("blob stream must exist when response.found is true");

                debug!(%peer_id, "Starting to stream blob chunks");

                let mut chunk_count = 0usize;
                let mut total_bytes_sent = 0usize;

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

                            let chunk_data =
                                borsh::to_vec(&blob_chunk).wrap_err("serialize blob chunk")?;

                            timeout(
                                CHUNK_SEND_TIMEOUT,
                                stream.send(StreamMessage::new(chunk_data)),
                            )
                            .await
                            .map_err(|_| eyre::eyre!("Timeout sending chunk {chunk_count}"))??;

                            if chunk_count % 10 == 0 {
                                sleep(FLOW_CONTROL_DELAY).await;
                            }
                        }
                        Err(e) => {
                            warn!(%peer_id, error = %e, "Failed to read blob chunk");
                            return Err(eyre::eyre!("Failed to read blob chunk: {e}"));
                        }
                    }
                }

                let final_chunk = BlobChunk { data: Vec::new() };
                let final_chunk_data =
                    borsh::to_vec(&final_chunk).wrap_err("serialize final chunk")?;

                timeout(
                    CHUNK_SEND_TIMEOUT,
                    stream.send(StreamMessage::new(final_chunk_data)),
                )
                .await
                .map_err(|_| eyre::eyre!("Timeout sending final chunk"))??;

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

    pub(crate) async fn handle_blob_share_request(
        &self,
        context: &Context,
        our_identity: PublicKey,
        their_identity: PublicKey,
        blob_id: BlobId,
        stream: &mut Stream,
    ) -> Result<()> {
        blob_share::handle_blob_share_request(
            &self.node_client,
            &self.context_client,
            context,
            our_identity,
            their_identity,
            blob_id,
            stream,
        )
        .await
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct BlobRequest {
    blob_id: BlobId,
    context_id: ContextId,
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
