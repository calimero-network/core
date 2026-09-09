//! "I now hold this blob for this context" — one peer, one message, no reply.
//!
//! Runs on [`CALIMERO_BLOB_ANNOUNCE_PROTOCOL`], not on the transfer protocol:
//! that one parses its first frame strictly as a `BlobRequest`, so a new
//! message kind there would force a version bump on the whole transfer path.
//!
//! The announce is addressed to ONE peer per call, and the caller sends it only
//! to the context's availability nodes. That is deliberate — gossipsub is not
//! an option here, because `flood_publish` fans every publish to every
//! subscriber of the topic, which is the fan-out this whole design avoids.

use core::time::Duration;

use actix::{Context, Handler, Message, ResponseFuture};
use calimero_network_primitives::{
    blob_types::BlobAnnouncement,
    messages::SendBlobAnnouncement,
    stream::{Message as StreamMessage, Stream, CALIMERO_BLOB_ANNOUNCE_PROTOCOL},
};
use futures_util::SinkExt;
use tracing::debug;

use crate::NetworkManager;

/// A single frame with no response, so it gets the probe's tight budget rather
/// than a transfer's. The caller treats a failure as "that node missed this
/// blob", never as a failure of the write that produced it.
const ANNOUNCE_TIMEOUT: Duration = Duration::from_secs(5);

impl Handler<SendBlobAnnouncement> for NetworkManager {
    type Result = ResponseFuture<<SendBlobAnnouncement as Message>::Result>;

    fn handle(&mut self, request: SendBlobAnnouncement, _ctx: &mut Context<Self>) -> Self::Result {
        let mut stream_control = self.swarm.behaviour().stream.new_control();

        Box::pin(async move {
            let announced = tokio::time::timeout(ANNOUNCE_TIMEOUT, async {
                let libp2p_stream = stream_control
                    .open_stream(request.peer_id, CALIMERO_BLOB_ANNOUNCE_PROTOCOL)
                    .await?;
                let mut stream = Stream::new(libp2p_stream);

                let announcement = BlobAnnouncement {
                    blob_id: request.blob_id,
                    context_id: request.context_id,
                    size: request.size,
                };
                stream
                    .send(StreamMessage::new(serde_json::to_vec(&announcement)?))
                    .await?;

                // Close rather than drop, so the receiver sees a clean
                // end-of-stream instead of a reset after the one frame.
                stream.close().await?;

                Ok::<(), eyre::Report>(())
            })
            .await;

            match announced {
                Ok(Ok(())) => {
                    debug!(
                        peer_id = %request.peer_id,
                        blob_id = %request.blob_id,
                        context_id = %request.context_id,
                        "announced blob to availability node"
                    );
                    Ok(())
                }
                Ok(Err(err)) => {
                    debug!(peer_id = %request.peer_id, %err, "blob announce failed");
                    Err(err)
                }
                Err(_elapsed) => {
                    debug!(
                        peer_id = %request.peer_id,
                        timeout_secs = ANNOUNCE_TIMEOUT.as_secs(),
                        "blob announce timed out"
                    );
                    Err(eyre::eyre!("blob announce timed out"))
                }
            }
        })
    }
}
