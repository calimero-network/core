use actix::{AsyncContext, WrapFuture};
use calimero_primitives::{blobs::BlobId, context::ContextId};
use libp2p::PeerId;
use tracing::{debug, error, info};

use crate::NodeManager;

pub(super) fn handle_blob_requested(
    blob_id: BlobId,
    context_id: ContextId,
    requesting_peer: PeerId,
) {
    debug!(
        blob_id = %blob_id,
        context_id = %context_id,
        requesting_peer = %requesting_peer,
        "Blob requested by peer"
    );
}

pub(super) fn handle_blob_providers_found(
    blob_id: BlobId,
    context_id: Option<ContextId>,
    providers: Vec<PeerId>,
) {
    debug!(
        blob_id = %blob_id,
        context_id = ?context_id.as_ref().map(|id| id.to_string()),
        providers_count = providers.len(),
        "Blob providers found in DHT"
    );
}

pub(super) fn handle_blob_downloaded(
    manager: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    blob_id: BlobId,
    context_id: ContextId,
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

    let blobstore = manager.managers.blobstore.clone();
    let blob_data = data;

    let _ignored = ctx.spawn(
        async move {
            let reader = &blob_data[..];

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
        .into_actor(manager),
    );
}

pub(super) fn handle_blob_download_failed(
    blob_id: BlobId,
    context_id: ContextId,
    from_peer: PeerId,
    error_message: String,
) {
    info!(
        blob_id = %blob_id,
        context_id = %context_id,
        from_peer = %from_peer,
        error = %error_message,
        "Blob download failed"
    );
}
