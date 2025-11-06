use actix::{AsyncContext, WrapFuture};
use calimero_network_primitives::stream::Stream;
use libp2p::{PeerId, StreamProtocol};
use tracing::{debug, error, info};

use crate::comms::direct::streams;
use crate::NodeManager;

pub fn handle_stream_opened(
    node_manager: &mut NodeManager,
    ctx: &mut <NodeManager as actix::Actor>::Context,
    peer_id: PeerId,
    stream: Box<Stream>,
    protocol: StreamProtocol,
) {
    streams::handle_stream_opened(node_manager, ctx, peer_id, stream, protocol);
}

pub fn handle_blob_requested(
    blob_id: calimero_primitives::blobs::BlobId,
    context_id: calimero_primitives::context::ContextId,
    requesting_peer: PeerId,
) {
    debug!(
        blob_id = %blob_id,
        context_id = %context_id,
        requesting_peer = %requesting_peer,
        "Blob requested by peer"
    );
}

pub fn handle_blob_providers_found(
    blob_id: calimero_primitives::blobs::BlobId,
    context_id: Option<calimero_primitives::context::ContextId>,
    providers: Vec<PeerId>,
) {
    debug!(
        blob_id = %blob_id,
        context_id = ?context_id.as_ref().map(|id| id.to_string()),
        providers_count = providers.len(),
        "Blob providers found in DHT"
    );
}

pub fn handle_blob_downloaded(
    node_manager: &mut NodeManager,
    ctx: &mut <NodeManager as actix::Actor>::Context,
    blob_id: calimero_primitives::blobs::BlobId,
    context_id: calimero_primitives::context::ContextId,
    data: Vec<u8>,
    from_peer: PeerId,
) {
    info!(
        blob_id = %blob_id,
        context_id = %context_id,
        from_peer = %from_peer,
        data_size = data.len(),
        "Blob downloaded successfully from peer"
    );

    let blobstore = node_manager.managers.blobstore.clone();
    ctx.spawn(
        async move {
            let reader = &data[..];

            match blobstore.put(reader).await {
                Ok((stored_blob_id, _hash, size)) => {
                    info!(
                        requested_blob_id = %blob_id,
                        stored_blob_id = %stored_blob_id,
                        size = size,
                        "Blob stored successfully"
                    );
                }
                Err(e) => {
                    error!(
                        blob_id = %blob_id,
                        error = %e,
                        "Failed to store downloaded blob"
                    );
                }
            }
        }
        .into_actor(node_manager),
    );
}

pub fn handle_blob_download_failed(
    blob_id: calimero_primitives::blobs::BlobId,
    context_id: calimero_primitives::context::ContextId,
    from_peer: PeerId,
    error_msg: String,
) {
    info!(
        blob_id = %blob_id,
        context_id = %context_id,
        from_peer = %from_peer,
        error = %error_msg,
        "Blob download failed"
    );
}
