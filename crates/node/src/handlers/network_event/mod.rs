//! Network event handlers
//!
//! Delegates work to specialised broadcast and direct modules to keep this
//! entrypoint focused on wiring Actix messages.

mod broadcast;
mod direct;

use actix::Handler;
use calimero_network_primitives::messages::NetworkEvent;
use calimero_primitives::context::ContextId;
use tracing::{debug, info};

use crate::NodeManager;

impl Handler<NetworkEvent> for NodeManager {
    type Result = <NetworkEvent as actix::Message>::Result;

    fn handle(&mut self, msg: NetworkEvent, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            NetworkEvent::ListeningOn { address, .. } => {
                info!("Listening on: {}", address);
            }
            NetworkEvent::Subscribed { peer_id, topic } => {
                let Ok(context_id): Result<ContextId, _> = topic.as_str().parse() else {
                    return;
                };

                if !self
                    .clients
                    .context
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
            NetworkEvent::Unsubscribed { peer_id, topic } => {
                let Ok(context_id): Result<ContextId, _> = topic.as_str().parse() else {
                    return;
                };

                info!(
                    "Peer '{}' unsubscribed from context '{}'",
                    peer_id, context_id
                );
            }
            NetworkEvent::Message { message, .. } => {
                broadcast::handle_message(self, ctx, message);
            }
            NetworkEvent::StreamOpened {
                peer_id,
                stream,
                protocol,
            } => {
                direct::handle_stream_opened(self, ctx, peer_id, stream, protocol);
            }
            NetworkEvent::BlobRequested {
                blob_id,
                context_id,
                requesting_peer,
            } => {
                direct::handle_blob_requested(blob_id, context_id, requesting_peer);
            }
            NetworkEvent::BlobProvidersFound {
                blob_id,
                context_id,
                providers,
            } => {
                direct::handle_blob_providers_found(blob_id, context_id, providers);
            }
            NetworkEvent::BlobDownloaded {
                blob_id,
                context_id,
                data,
                from_peer,
            } => {
                direct::handle_blob_downloaded(self, ctx, blob_id, context_id, data, from_peer);
            }
            NetworkEvent::BlobDownloadFailed {
                blob_id,
                context_id,
                from_peer,
                error,
            } => {
                direct::handle_blob_download_failed(blob_id, context_id, from_peer, error);
            }
        }
    }
}
