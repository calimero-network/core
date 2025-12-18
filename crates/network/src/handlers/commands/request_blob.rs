use core::time::Duration;

use actix::{Context, Handler, Message, ResponseFuture};
use borsh::BorshDeserialize;
use calimero_network_primitives::{
    blob_types::{BlobChunk, BlobRequest, BlobResponse},
    messages::{NetworkEvent, RequestBlob},
    stream::{Message as StreamMessage, Stream, CALIMERO_BLOB_PROTOCOL},
};
use eyre::{eyre, Context as EyreContext};
use futures_util::{SinkExt, StreamExt};
use tokio::time::timeout;
use tracing::debug;

use crate::NetworkManager;

// Timeout for individual operations during blob transfer
const BLOB_TRANSFER_TIMEOUT: Duration = Duration::from_secs(60); // 1 minute per operation
const CHUNK_RECEIVE_TIMEOUT: Duration = Duration::from_secs(30); // 30 seconds per chunk

impl Handler<RequestBlob> for NetworkManager {
    type Result = ResponseFuture<<RequestBlob as Message>::Result>;

    fn handle(&mut self, request: RequestBlob, _ctx: &mut Context<Self>) -> Self::Result {
        debug!(
            blob_id = %request.blob_id,
            context_id = %request.context_id,
            peer_id = %request.peer_id,
            auth = ?request.auth,
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
                            error: format!("Failed to open stream: {e}"),
                        });
                        return Err(e).wrap_err("Failed to open stream to peer");
                    }
                };

                // Convert to Calimero stream
                let mut stream = Stream::new(libp2p_stream);

                // Send blob request
                let blob_request = BlobRequest {
                    blob_id: request.blob_id,
                    context_id: request.context_id,
                    auth: request.auth,
                };

                let request_data = match serde_json::to_vec(&blob_request) {
                    Ok(data) => data,
                    Err(e) => {
                        // Emit failure event
                        event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                            blob_id: request.blob_id,
                            context_id: request.context_id,
                            from_peer: request.peer_id,
                            error: format!("Failed to serialize request: {e}"),
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
                        error: format!("Failed to send request: {e}"),
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
                            error: format!("Failed to receive response: {e}"),
                        });
                        return Err(e).wrap_err("Failed to receive response");
                    }
                    Ok(None) => {
                        // Emit failure event
                        event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                            blob_id: request.blob_id,
                            context_id: request.context_id,
                            from_peer: request.peer_id,
                            error: "Stream closed without response".to_owned(),
                        });
                        return Err(eyre!("Stream closed without response"));
                    }
                    Err(_) => {
                        // Timeout occurred
                        event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                            blob_id: request.blob_id,
                            context_id: request.context_id,
                            from_peer: request.peer_id,
                            error: "Timeout waiting for response".to_owned(),
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
                            error: format!("Failed to deserialize response: {e}"),
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
                        "Blob found, starting streaming download"
                    );

                    // Collect chunks efficiently (no pre-allocation based on expected size)
                    let mut collected_data = Vec::new();
                    let mut chunk_count = 0;
                    let start_time = std::time::Instant::now();

                    debug!(
                        blob_id = %request.blob_id,
                        peer_id = %request.peer_id,
                        expected_size = ?blob_response.size,
                        "Starting blob download"
                    );

                    // Process chunks and collect data
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
                                event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                                    blob_id: request.blob_id,
                                    context_id: request.context_id,
                                    from_peer: request.peer_id,
                                    error: format!("Stream error while receiving chunk: {e}"),
                                });
                                return Err(e).wrap_err("Stream error while receiving chunk");
                            }
                            Ok(None) => {
                                // Stream closed - this is natural EOF
                                debug!(
                                    blob_id = %request.blob_id,
                                    peer_id = %request.peer_id,
                                    total_chunks = chunk_count,
                                    total_size = collected_data.len(),
                                    "Stream closed, blob transfer complete"
                                );
                                break;
                            }
                            Err(_) => {
                                event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                                    blob_id: request.blob_id,
                                    context_id: request.context_id,
                                    from_peer: request.peer_id,
                                    error: format!("Timeout waiting for chunk {}", chunk_count + 1),
                                });
                                return Err(eyre!("Timeout waiting for chunk"));
                            }
                        };

                        let blob_chunk = match BlobChunk::try_from_slice(&chunk_msg.data) {
                            Ok(chunk) => {
                                let chunk_size: usize = chunk.data.len();
                                debug!("Successfully parsed borsh chunk: blob_id={}, peer_id={}, chunk_number={}, chunk_size={}",
                                    request.blob_id,
                                    request.peer_id,
                                    chunk_count + 1,
                                    chunk_size
                                );
                                chunk
                            },
                            Err(e) => {
                                // Log raw data for debugging
                                debug!(
                                    blob_id = %request.blob_id,
                                    peer_id = %request.peer_id,
                                    chunk_number = chunk_count + 1,
                                    raw_data_size = chunk_msg.data.len(),
                                    raw_data_hex = hex::encode(&chunk_msg.data[..core::cmp::min(100, chunk_msg.data.len())]),
                                    error = %e,
                                    "Failed to parse chunk"
                                );
                                event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                                    blob_id: request.blob_id,
                                    context_id: request.context_id,
                                    from_peer: request.peer_id,
                                    error: format!("Failed to parse chunk: {e}"),
                                });
                                return Err(eyre::eyre!("Failed to parse blob chunk: {}", e));
                            }
                        };

                        // Get chunk size before moving the data
                        let chunk_size = blob_chunk.data.len();
                        // Add chunk data to collection (move semantics, no clone)
                        collected_data.extend(blob_chunk.data);
                        chunk_count += 1;

                        debug!(
                            blob_id = %request.blob_id,
                            peer_id = %request.peer_id,
                            chunk_number = chunk_count,
                            chunk_size = chunk_size,
                            total_received = collected_data.len(),
                            "Received blob chunk"
                        );

                        // Continue receiving chunks until stream closes
                    }

                    // Calculate and log transfer statistics
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
                        "Blob download completed successfully"
                    );

                    // Emit success event for NodeManager to handle storage
                    event_recipient.do_send(NetworkEvent::BlobDownloaded {
                        blob_id: request.blob_id,
                        context_id: request.context_id,
                        data: collected_data.clone(),
                        from_peer: request.peer_id,
                    });

                    // Return the collected data
                    Ok(Some(collected_data))
                } else {
                    // Emit failure event - blob not found
                    event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                        blob_id: request.blob_id,
                        context_id: request.context_id,
                        from_peer: request.peer_id,
                        error: "Blob not found on peer".to_owned(),
                    });
                    Ok(None)
                }
            }).await;

            // Handle timeout result
            if let Ok(result) = transfer_result {
                result
            } else {
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_borsh_serialization() {
        let test_cases = vec![
            // Basic case
            BlobChunk {
                data: vec![1, 2, 3, 4, 5],
            },
            // Empty data
            BlobChunk { data: vec![] },
            // Large data
            BlobChunk {
                data: vec![42; 1024], // 1KB of data
            },
            // Single byte chunk
            BlobChunk { data: vec![1] },
        ];

        for (i, chunk) in test_cases.into_iter().enumerate() {
            let bytes = borsh::to_vec(&chunk)
                .unwrap_or_else(|e| panic!("Failed to serialize chunk {}: {}", i, e));

            let parsed = BlobChunk::try_from_slice(&bytes)
                .unwrap_or_else(|e| panic!("Failed to deserialize chunk {}: {}", i, e));

            assert_eq!(chunk.data, parsed.data, "Data should match for chunk {}", i);

            // Verify the serialized size is correct
            assert_eq!(
                bytes.len(),
                // Borsh format: vec length (u32) + vec data
                4 + chunk.data.len(),
                "Serialized size should match expected for chunk {}",
                i
            );
        }
    }
}
