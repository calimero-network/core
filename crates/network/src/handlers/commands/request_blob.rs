use actix::{Context, Handler, Message, ResponseFuture};
use calimero_network_primitives::messages::{RequestBlob, NetworkEvent};
use calimero_network_primitives::stream::{Message as StreamMessage, Stream, CALIMERO_BLOB_PROTOCOL};
use eyre::{eyre, Context as EyreContext};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::NetworkManager;

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobRequest {
    pub blob_id: [u8; 32],
    pub context_id: [u8; 32],
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobResponse {
    pub found: bool,
    pub data: Option<Vec<u8>>,
}

impl Handler<RequestBlob> for NetworkManager {
    type Result = ResponseFuture<<RequestBlob as Message>::Result>;

    fn handle(&mut self, request: RequestBlob, _ctx: &mut Context<Self>) -> Self::Result {
        debug!(
            blob_id = %request.blob_id,
            context_id = %request.context_id,
            peer_id = %request.peer_id,
            "Requesting blob from peer"
        );

        let mut stream_control = self.swarm.behaviour().stream.new_control();
        let event_recipient = self.event_recipient.clone();

        Box::pin(async move {
            // Open a stream to the peer
            let libp2p_stream = match stream_control
                .open_stream(request.peer_id, CALIMERO_BLOB_PROTOCOL)
                .await {
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

            // Wait for response
            let response_msg = match stream.next().await {
                Some(Ok(msg)) => msg,
                Some(Err(e)) => {
                    // Emit failure event
                    event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                        blob_id: request.blob_id,
                        context_id: request.context_id,
                        from_peer: request.peer_id,
                        error: format!("Failed to receive response: {}", e),
                    });
                    return Err(e).wrap_err("Failed to receive response");
                }
                None => {
                    // Emit failure event
                    event_recipient.do_send(NetworkEvent::BlobDownloadFailed {
                        blob_id: request.blob_id,
                        context_id: request.context_id,
                        from_peer: request.peer_id,
                        error: "Stream closed without response".to_string(),
                    });
                    return Err(eyre!("Stream closed without response"));
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

            if blob_response.found {
                if let Some(data) = blob_response.data.clone() {
                    // Emit success event
                    event_recipient.do_send(NetworkEvent::BlobDownloaded {
                        blob_id: request.blob_id,
                        context_id: request.context_id,
                        data: data.clone(),
                        from_peer: request.peer_id,
                    });
                }
                Ok(blob_response.data)
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
        })
    }
} 