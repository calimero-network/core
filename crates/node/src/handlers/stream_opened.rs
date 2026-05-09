//! StreamOpened event handling
//!
//! **SRP**: This module has ONE job - route incoming streams to the correct handler

use actix::{AsyncContext, WrapFuture};
use calimero_network_primitives::stream::Stream;
use libp2p::{PeerId, StreamProtocol};
use tracing::{debug, info, warn};

use crate::handlers::blob_protocol::handle_blob_protocol_stream;
use crate::sync_session_bridge::{SyncSessionJob, SyncSessionSendError};
use crate::NodeManager;

/// Handles StreamOpened event by routing to blob or sync protocol
///
/// Protocol routing:
/// - `CALIMERO_BLOB_PROTOCOL` → blob_protocol::handle_blob_protocol_stream
/// - All other protocols → SyncManager::handle_opened_stream
pub fn handle_stream_opened(
    node_manager: &mut NodeManager,
    ctx: &mut <NodeManager as actix::Actor>::Context,
    peer_id: PeerId,
    stream: Box<Stream>,
    protocol: StreamProtocol,
) {
    // Route streams based on protocol
    if protocol == calimero_network_primitives::stream::CALIMERO_BLOB_PROTOCOL {
        info!(%peer_id, "Routing to blob protocol handler");
        let node_client = node_manager.clients.node.clone();
        let context_client = node_manager.clients.context.clone();
        let _ignored = ctx.spawn(
            async move {
                if let Err(err) =
                    handle_blob_protocol_stream(node_client, context_client, peer_id, stream).await
                {
                    debug!(%peer_id, error = %err, "Failed to handle blob protocol stream");
                }
            }
            .into_actor(node_manager),
        );
    } else {
        debug!(%peer_id, "Routing to sync protocol handler");
        // Route inbound sync streams onto the dedicated SyncSessionActor
        // arbiter (issue #2316). On Full/Closed we drop the stream and
        // rely on peer retry; the bounded mailbox is the whole point of
        // moving sync sessions off this actor's arbiter.
        match node_manager
            .sync_session_tx
            .try_send(SyncSessionJob::Responder { peer_id, stream })
        {
            Ok(()) => {}
            Err(SyncSessionSendError::Full) => {
                warn!(
                    %peer_id,
                    "SyncSession actor mailbox full — dropping inbound sync stream (#2316); peer will retry"
                );
            }
            Err(SyncSessionSendError::Closed) => {
                warn!(
                    %peer_id,
                    "SyncSession actor closed — dropping inbound sync stream"
                );
            }
        }
    }
}
