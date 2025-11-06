//! Routing and responders for direct libp2p streams.
//!
//! `handle_stream_opened` acts as the narrow integration point from the
//! network layer into the sync subsystem. It simply routes incoming streams to
//! either the blob responder (`BlobResponder`) or the sync responder
//! (`SyncResponder`). The concrete responders live in submodules to keep this
//! file focused on the orchestration logic.

mod blob;
mod sync;

pub(crate) use blob::BlobResponder;
pub(crate) use sync::SyncResponder;

use actix::{AsyncContext, WrapFuture};
use calimero_network_primitives::stream::Stream;
use libp2p::{PeerId, StreamProtocol};
use tracing::{debug, info};

use crate::NodeManager;

/// Routes a newly-opened libp2p stream to either the blob protocol or the
/// general sync responder.
pub fn handle_stream_opened(
    node_manager: &mut NodeManager,
    ctx: &mut <NodeManager as actix::Actor>::Context,
    peer_id: PeerId,
    stream: Box<Stream>,
    protocol: StreamProtocol,
) {
    if protocol == calimero_network_primitives::stream::CALIMERO_BLOB_PROTOCOL {
        info!(%peer_id, "Routing to blob protocol responder");
        let responder = BlobResponder::new(
            node_manager.clients.node.clone(),
            node_manager.clients.context.clone(),
        );
        let _ignored = ctx.spawn(
            async move {
                if let Err(err) = responder.handle_stream(peer_id, stream).await {
                    debug!(%peer_id, error = %err, "Failed to handle blob protocol stream");
                }
            }
            .into_actor(node_manager),
        );
    } else {
        debug!(%peer_id, "Routing to sync protocol responder");
        let sync_manager = node_manager.managers.sync.clone();
        let _ignored = ctx.spawn(
            async move {
                sync_manager.handle_opened_stream(stream).await;
            }
            .into_actor(node_manager),
        );
    }
}
