use calimero_primitives::{blobs::BlobId, context::ContextId};
use libp2p::PeerId;
use tracing::{debug, info};

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

/// A blob arrived from a peer. Notification only — this must NOT store it.
///
/// The requesting path already does
/// (`calimero_node_primitives::client::blob::get_blob`): it stores the download
/// and then serves it out of the store, so storage cannot be delegated to a
/// detached task here. `NetworkClient::request_blob` — the sole thing that emits
/// this event — has exactly one caller in the tree, so there is no unsolicited
/// download that would otherwise go unstored.
///
/// Storing here as well is what produced the intermittent
/// `blob chunk hash mismatch; refusing to serve` in the `blob-cross-node-sizes`
/// e2e: two writers racing on one chunk while the HTTP handler streamed it. The
/// content check that used to live here is not lost — the requesting path makes
/// the same one against the id it asked for, and refuses the peer's answer on a
/// mismatch.
///
/// For the same reason this is also the wrong place to react to a blob becoming
/// usable — an application arriving, say. This event fires when the bytes finish
/// transferring, which is *before* the requesting path stores them, so anything
/// checking whether the blob is present would race and usually lose. React at
/// the point the requesting path has stored it instead.
pub(super) fn handle_blob_downloaded(
    blob_id: BlobId,
    context_id: ContextId,
    size: usize,
    from_peer: PeerId,
) {
    info!(
        blob_id = %blob_id,
        context_id = %context_id,
        from_peer = %from_peer,
        data_size = size,
        "Blob downloaded successfully from peer"
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
