use std::time::Duration;

use actix::{Context, Handler, Message, ResponseFuture};
use calimero_network_primitives::messages::{NetworkEvent, RequestBlob};
use calimero_network_primitives::stream::{
    Message as StreamMessage, Stream, CALIMERO_BLOB_PROTOCOL,
};
use eyre::{eyre, Context as EyreContext};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::NetworkManager;

// Timeout for individual operations during blob transfer
const BLOB_TRANSFER_TIMEOUT: Duration = Duration::from_secs(60); // 1 minute per operation
const CHUNK_RECEIVE_TIMEOUT: Duration = Duration::from_secs(30); // 30 seconds per chunk

// Self-test for binary protocol
fn test_binary_protocol() -> eyre::Result<()> {
    let test_chunk = BlobChunk {
        data: vec![1, 2, 3, 4, 5],
        is_final: true,
    };
    let bytes = test_chunk.to_bytes();
    let parsed = BlobChunk::from_bytes(&bytes)?;

    if test_chunk.data != parsed.data || test_chunk.is_final != parsed.is_final {
        return Err(eyre::eyre!("Binary protocol self-test failed"));
    }

    tracing::debug!("Binary protocol self-test passed");
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobRequest {
    pub blob_id: [u8; 32],
    pub context_id: [u8; 32],
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobResponse {
    pub found: bool,
    pub size: Option<u64>, // Total size if found
}

// Use a more efficient binary format for chunk transfer
#[derive(Debug)]
pub struct BlobChunk {
    pub data: Vec<u8>,
    pub is_final: bool,
}

impl BlobChunk {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.data.len() + 9); // 8 bytes for length + 1 byte for is_final
        bytes.extend_from_slice(&(self.data.len() as u64).to_le_bytes());
        bytes.push(if self.is_final { 1u8 } else { 0u8 });
        bytes.extend_from_slice(&self.data);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> eyre::Result<Self> {
        if bytes.len() < 9 {
            return Err(eyre::eyre!("Invalid chunk data: too short"));
        }

        let data_len = u64::from_le_bytes(bytes[0..8].try_into()?) as usize;
        let is_final = bytes[8] != 0;

        if bytes.len() != 9 + data_len {
            return Err(eyre::eyre!("Invalid chunk data: length mismatch"));
        }

        let data = bytes[9..].to_vec();

        Ok(Self { data, is_final })
    }
}

impl Handler<RequestBlob> for NetworkManager {
    type Result = ResponseFuture<<RequestBlob as Message>::Result>;

    fn handle(&mut self, request: RequestBlob, _ctx: &mut Context<Self>) -> Self::Result {
        // Test binary protocol on first use
        if let Err(e) = test_binary_protocol() {
            warn!("Binary protocol test failed: {}", e);
        }

        debug!(
            blob_id = %request.blob_id,
            context_id = %request.context_id,
            peer_id = %request.peer_id,
            "Requesting blob from peer using binary chunk protocol"
        );

        let mut stream_control = self.swarm.behaviour().stream.new_control();
        let event_recipient = self.event_recipient.clone();

        Box::pin(async move {
            // Wrap the entire blob transfer in a timeout
            let transfer_result = timeout(BLOB_TRANSFER_TIMEOUT, async {
                // Open a stream to the peer
                let libp2p_stream = match stream_control
                    .open_stream(request.peer_id, CALIMERO_BLOB_PROTOCOL)
                    .await
                {
                    Ok(stream) => stream,
                    Err(e) => {
                        // Emit failure event
                        event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                            blob_id: request.blob_id,
                            context_id: request.context_id,
                            from_peer: request.peer_id,
                            error: format!("Failed to open stream: {}", e),
                        });
                        return Err(e).wrap_err("Failed to open stream to peer");
                    }
                };

                // Convert to Calimero stream
                let mut stream = Stream::new(libp2p_stream);

                // Send blob request
                let blob_request = BlobRequest {
                    blob_id: *request.blob_id,
                    context_id: *request.context_id,
                };

                let request_data = match serde_json::to_vec(&blob_request) {
                    Ok(data) => data,
                    Err(e) => {
                        // Emit failure event
                        event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                            blob_id: request.blob_id,
                            context_id: request.context_id,
                            from_peer: request.peer_id,
                            error: format!("Failed to serialize request: {}", e),
                        });
                        return Err(e).wrap_err("Failed to serialize blob request");
                    }
                };

                if let Err(e) = stream.send(StreamMessage::new(request_data)).await {
                    // Emit failure event
                    event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                        blob_id: request.blob_id,
                        context_id: request.context_id,
                        from_peer: request.peer_id,
                        error: format!("Failed to send request: {}", e),
                    });
                    return Err(e).wrap_err("Failed to send blob request");
                }

                // Wait for initial response with timeout
                let response_msg = match timeout(CHUNK_RECEIVE_TIMEOUT, stream.next()).await {
                    Ok(Some(Ok(msg))) => msg,
                    Ok(Some(Err(e))) => {
                        // Emit failure event
                        event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                            blob_id: request.blob_id,
                            context_id: request.context_id,
                            from_peer: request.peer_id,
                            error: format!("Failed to receive response: {}", e),
                        });
                        return Err(e).wrap_err("Failed to receive response");
                    }
                    Ok(None) => {
                        // Emit failure event
                        event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                            blob_id: request.blob_id,
                            context_id: request.context_id,
                            from_peer: request.peer_id,
                            error: "Stream closed without response".to_string(),
                        });
                        return Err(eyre!("Stream closed without response"));
                    }
                    Err(_) => {
                        // Timeout occurred
                        event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                            blob_id: request.blob_id,
                            context_id: request.context_id,
                            from_peer: request.peer_id,
                            error: "Timeout waiting for response".to_string(),
                        });
                        return Err(eyre!("Timeout waiting for response"));
                    }
                };

                let blob_response: BlobResponse = match serde_json::from_slice(&response_msg.data) {
                    Ok(response) => response,
                    Err(e) => {
                        // Emit failure event
                        event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                            blob_id: request.blob_id,
                            context_id: request.context_id,
                            from_peer: request.peer_id,
                            error: format!("Failed to deserialize response: {}", e),
                        });
                        return Err(e).wrap_err("Failed to deserialize blob response");
                    }
                };

                debug!(
                    blob_id = %request.blob_id,
                    peer_id = %request.peer_id,
                    found = blob_response.found,
                    size = ?blob_response.size,
                    "Received initial blob response"
                );

                if blob_response.found {
                    debug!(
                        blob_id = %request.blob_id,
                        context_id = %request.context_id,
                        peer_id = %request.peer_id,
                        size = ?blob_response.size,
                        "Blob found, streaming chunks"
                    );

                    // Prepare to collect chunks
                    let expected_size = blob_response.size.unwrap_or(0);
                    let mut collected_data = Vec::with_capacity(expected_size as usize);
                    let mut chunk_count = 0;
                    let start_time = std::time::Instant::now();

                    debug!(
                        blob_id = %request.blob_id,
                        peer_id = %request.peer_id,
                        expected_size,
                        "Starting chunked blob download"
                    );

                    // Stream chunks until we get is_final=true
                    loop {
                        debug!(
                            blob_id = %request.blob_id,
                            peer_id = %request.peer_id,
                            chunk_number = chunk_count + 1,
                            "Waiting for next chunk"
                        );

                        let chunk_msg = match timeout(CHUNK_RECEIVE_TIMEOUT, stream.next()).await {
                            Ok(Some(Ok(msg))) => {
                                debug!(
                                    blob_id = %request.blob_id,
                                    peer_id = %request.peer_id,
                                    chunk_number = chunk_count + 1,
                                    message_size = msg.data.len(),
                                    "Received raw chunk message"
                                );
                                msg
                            },
                            Ok(Some(Err(e))) => {
                                // Emit failure event
                                event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                                    blob_id: request.blob_id,
                                    context_id: request.context_id,
                                    from_peer: request.peer_id,
                                    error: format!("Stream error while receiving chunk: {}", e),
                                });
                                return Err(e).wrap_err("Stream error while receiving chunk");
                            }
                            Ok(None) => {
                                // Emit failure event
                                event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                                    blob_id: request.blob_id,
                                    context_id: request.context_id,
                                    from_peer: request.peer_id,
                                    error: "Stream closed during chunk transfer".to_string(),
                                });
                                return Err(eyre!("Stream closed during chunk transfer"));
                            }
                            Err(_) => {
                                // Timeout occurred
                                event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                                    blob_id: request.blob_id,
                                    context_id: request.context_id,
                                    from_peer: request.peer_id,
                                    error: format!("Timeout waiting for chunk {} (received {} bytes so far)", chunk_count + 1, collected_data.len()),
                                });
                                return Err(eyre!("Timeout waiting for chunk"));
                            }
                        };

                        let blob_chunk: BlobChunk = match BlobChunk::from_bytes(&chunk_msg.data) {
                            Ok(chunk) => {
                                debug!(
                                    blob_id = %request.blob_id,
                                    peer_id = %request.peer_id,
                                    chunk_number = chunk_count + 1,
                                    chunk_data_size = chunk.data.len(),
                                    is_final = chunk.is_final,
                                    "Successfully parsed binary chunk"
                                );
                                chunk
                            },
                            Err(binary_error) => {
                                // Try fallback to JSON parsing in case server is still using old format
                                debug!(
                                    blob_id = %request.blob_id,
                                    peer_id = %request.peer_id,
                                    chunk_number = chunk_count + 1,
                                    binary_error = %binary_error,
                                    "Binary parsing failed, trying JSON fallback"
                                );

                                #[derive(Debug, serde::Deserialize)]
                                struct JsonBlobChunk {
                                    data: Vec<u8>,
                                    is_final: bool,
                                }

                                match serde_json::from_slice::<JsonBlobChunk>(&chunk_msg.data) {
                                    Ok(json_chunk) => {
                                        warn!(
                                            blob_id = %request.blob_id,
                                            peer_id = %request.peer_id,
                                            chunk_number = chunk_count + 1,
                                            "Server is using old JSON format for chunks!"
                                        );
                                        BlobChunk {
                                            data: json_chunk.data,
                                            is_final: json_chunk.is_final,
                                        }
                                    },
                                    Err(json_error) => {
                                        // Log raw data for debugging
                                        debug!(
                                            blob_id = %request.blob_id,
                                            peer_id = %request.peer_id,
                                            chunk_number = chunk_count + 1,
                                            raw_data_size = chunk_msg.data.len(),
                                            raw_data_hex = hex::encode(&chunk_msg.data[..std::cmp::min(100, chunk_msg.data.len())]),
                                            binary_error = %binary_error,
                                            json_error = %json_error,
                                            "Failed to parse chunk as both binary and JSON - showing first 100 bytes as hex"
                                        );
                                        // Emit failure event
                                        event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                                            blob_id: request.blob_id,
                                            context_id: request.context_id,
                                            from_peer: request.peer_id,
                                            error: format!("Failed to parse chunk as binary ({}) or JSON ({})", binary_error, json_error),
                                        });
                                        return Err(eyre::eyre!("Failed to parse blob chunk as binary or JSON"));
                                    }
                                }
                            }
                        };

                        // Add chunk data to collection
                        collected_data.extend(blob_chunk.data.clone());
                        chunk_count += 1;

                        debug!(
                            blob_id = %request.blob_id,
                            peer_id = %request.peer_id,
                            chunk_number = chunk_count,
                            chunk_size = blob_chunk.data.len(),
                            total_received = collected_data.len(),
                            is_final = blob_chunk.is_final,
                            "Received blob chunk"
                        );

                        // Check if this is the final chunk
                        if blob_chunk.is_final {
                            let total_time = start_time.elapsed();
                            let transfer_rate = if total_time.as_secs() > 0 {
                                collected_data.len() as f64 / total_time.as_secs_f64() / (1024.0 * 1024.0) // MiB/s
                            } else {
                                0.0
                            };

                            debug!(
                                blob_id = %request.blob_id,
                                peer_id = %request.peer_id,
                                total_size = collected_data.len(),
                                total_chunks = chunk_count,
                                transfer_time_secs = total_time.as_secs_f64(),
                                transfer_rate_mibs = transfer_rate,
                                "Received final chunk, blob transfer complete"
                            );
                            break;
                        }
                    }

                    // Emit success event
                    event_recipient.do_send(NetworkEvent::BlobDownloaded {
                        blob_id: request.blob_id,
                        context_id: request.context_id,
                        data: collected_data.clone(),
                        from_peer: request.peer_id,
                    });

                    Ok(Some(collected_data))
                } else {
                    // Emit failure event - blob not found
                    event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                        blob_id: request.blob_id,
                        context_id: request.context_id,
                        from_peer: request.peer_id,
                        error: "Blob not found on peer".to_string(),
                    });
                    Ok(None)
                }
            }).await;

            // Handle timeout result
            match transfer_result {
                Ok(result) => result,
                Err(_) => {
                    // Overall transfer timeout
                    event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                        blob_id: request.blob_id,
                        context_id: request.context_id,
                        from_peer: request.peer_id,
                        error: format!(
                            "Blob transfer timed out after {} seconds",
                            BLOB_TRANSFER_TIMEOUT.as_secs()
                        ),
                    });
                    Err(eyre!("Blob transfer timed out"))
                }
            }
        })
    }
}
