//! "Do you have this blob?" — one peer, one round trip, no transfer.
//!
//! Reuses the existing `BlobRequest`/`BlobResponse` exchange: the server
//! answers `found` before streaming any chunk, so a probe is simply a request
//! whose chunks are never read. No new wire format.
//!
//! A probe deliberately does NOT distinguish "not authorised" from "not held" —
//! the current protocol answers both with `found: false`
//! (`crates/node/src/handlers/blob_protocol.rs`). Treating a refusal as absence
//! costs one wasted candidate, never a wrong fetch.

use core::time::Duration;

use actix::{Context, Handler, Message, ResponseFuture};
use calimero_network_primitives::{
    blob_types::{BlobRequest, BlobResponse},
    messages::ProbeBlob,
    stream::{Message as StreamMessage, Stream, CALIMERO_BLOB_PROTOCOL},
};
use futures_util::{SinkExt, StreamExt};
use tokio::time::timeout;
use tracing::debug;

use crate::NetworkManager;

/// A probe is a header exchange, so it gets a far tighter budget than a
/// transfer. A whole batch of these runs concurrently; none may hold up the
/// batch.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

impl Handler<ProbeBlob> for NetworkManager {
    type Result = ResponseFuture<<ProbeBlob as Message>::Result>;

    fn handle(&mut self, request: ProbeBlob, _ctx: &mut Context<Self>) -> Self::Result {
        let mut stream_control = self.swarm.behaviour().stream.new_control();

        Box::pin(async move {
            let probe = timeout(PROBE_TIMEOUT, async {
                let libp2p_stream = stream_control
                    .open_stream(request.peer_id, CALIMERO_BLOB_PROTOCOL)
                    .await?;
                let mut stream = Stream::new(libp2p_stream);

                let blob_request = BlobRequest {
                    blob_id: request.blob_id,
                    context_id: request.context_id,
                    auth: request.auth,
                };
                stream
                    .send(StreamMessage::new(serde_json::to_vec(&blob_request)?))
                    .await?;

                // A peer that closes without answering is not a holder. The
                // stream is dropped here, before any chunk is read: that is
                // what makes this a probe rather than a download.
                let Some(Ok(msg)) = stream.next().await else {
                    return Ok::<bool, eyre::Report>(false);
                };
                let response: BlobResponse = serde_json::from_slice(&msg.data)?;
                Ok(response.found)
            })
            .await;

            // Every failure mode collapses to "not a holder": the caller is
            // choosing among peers, and an unreachable or misbehaving one is
            // simply not the peer to fetch from. Returning an error here would
            // abort a search that has other candidates left.
            match probe {
                Ok(Ok(found)) => {
                    debug!(
                        peer_id = %request.peer_id,
                        blob_id = %request.blob_id,
                        found,
                        "blob probe complete"
                    );
                    Ok(found)
                }
                Ok(Err(err)) => {
                    debug!(peer_id = %request.peer_id, %err, "blob probe failed");
                    Ok(false)
                }
                Err(_elapsed) => {
                    debug!(
                        peer_id = %request.peer_id,
                        timeout_secs = PROBE_TIMEOUT.as_secs(),
                        "blob probe timed out"
                    );
                    Ok(false)
                }
            }
        })
    }
}
