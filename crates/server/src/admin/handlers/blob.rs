use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query};
use axum::http::response::Builder;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use calimero_node_primitives::client::{BlobPresence, BlobRejected};
use calimero_primitives::blobs::{BlobId, BlobInfo, BlobMetadata};
use calimero_primitives::content_hash::ContentHash;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use futures_util::{AsyncRead, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tokio_util::io::StreamReader;
use tracing::{debug, error, info};

use crate::admin::caller_scope::{admits_context, list_scope_for, ListScope};
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::auth::{AuthenticatedAccount, AuthenticatedDevice, AuthenticatedNodeOwner};
use crate::AdminState;

#[derive(Debug, Deserialize)]
pub struct BlobUploadQuery {
    /// Expected hash of the blob for verification
    hash: Option<String>,
    /// Context the blob is shared in; without one, peers are never served it
    context_id: Option<String>,
}

#[derive(Debug, Serialize, Copy, Clone)]
pub struct BlobUploadResponse {
    pub data: BlobInfo,
}

#[derive(Debug, Serialize)]
pub struct BlobListResponse {
    /// Wrapped response data
    pub data: BlobListResponseData,
}

#[derive(Debug, Serialize)]
pub struct BlobListResponseData {
    /// List of all blobs
    pub blobs: Vec<BlobInfo>,
}

#[derive(Debug, Serialize, Copy, Clone)]
pub struct BlobDeleteResponse {
    pub blob_id: BlobId,
    pub deleted: bool,
}

/// Hard ceiling on a single blob upload. The upload streams straight to blob
/// storage, so without a cap a client could stream an unbounded body and fill
/// the node's disk (there is no `Content-Length`-based limit — the body is
/// consumed as a stream). Enforced by counting bytes as they flow.
const MAX_BLOB_UPLOAD_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB

/// Ceiling on one upload by an account-scoped caller (a delegated session on a
/// relay). Far below [`MAX_BLOB_UPLOAD_BYTES`]: that limit protects a node
/// from its own operator's mistakes, this one protects a relay's disk from its
/// tenants. It bounds one request, not an account's total — see the PR's open
/// questions on quotas.
const MAX_ACCOUNT_BLOB_UPLOAD_BYTES: u64 = 64 * 1024 * 1024; // 64 MiB

const FETCH_SITE: &str = "sec-fetch-site"; // a browser sets both; a page can neither set nor drop them
const FETCH_MODE: &str = "sec-fetch-mode";

/// Whether a page on another site caused this request without a script's CORS request.
/// A navigation or a subresource load carries no `Origin`, so any site can issue one.
fn passive_from_another_site(headers: &HeaderMap) -> bool {
    headers
        .get(FETCH_SITE)
        .is_some_and(|site| site == "cross-site")
        && headers.get(FETCH_MODE).is_none_or(|mode| mode != "cors")
}

/// The context whose peers may be asked for a blob this node does not hold:
/// none for a request that must not cause network work or a store write.
fn peers_of(context_id: Option<ContextId>, headers: &HeaderMap) -> Option<ContextId> {
    context_id.filter(|_| !passive_from_another_site(headers))
}

/// Convert axum Body to futures AsyncRead using tokio_util::io::StreamReader.
/// This allows streaming large files without loading them entirely into memory.
///
/// The stream errors out once cumulative bytes exceed `limit` (the caller's
/// ceiling, [`MAX_BLOB_UPLOAD_BYTES`] or [`MAX_ACCOUNT_BLOB_UPLOAD_BYTES`]), so an
/// oversized (or unbounded/chunked) upload is aborted mid-stream rather
/// than being written to disk in full.
fn body_to_async_read(body: Body, limit: u64) -> impl AsyncRead {
    let mut total: u64 = 0;
    let byte_stream = body.into_data_stream().map(move |result| {
        let chunk = result.map_err(std::io::Error::other)?;
        total = total.saturating_add(chunk.len() as u64);
        if total > limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                UploadTooLarge,
            ));
        }
        Ok(chunk)
    });

    StreamReader::new(byte_stream).compat()
}

/// The upload stream's refusal once the body passes the caller's ceiling.
/// A type rather than a message so [`upload_refusal_status`] can find it in the
/// error `add_blob` returns and answer `413` instead of the generic `500`.
#[derive(Debug)]
struct UploadTooLarge;

impl std::fmt::Display for UploadTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("blob upload exceeds maximum allowed size")
    }
}

impl std::error::Error for UploadTooLarge {}

/// The status an upload failure answers with: `413` for a body over the limit,
/// `400` for bytes that are not the ones the caller described, and `500` only
/// for a real storage fault.
fn upload_refusal_status(err: &eyre::Report) -> StatusCode {
    if err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .is_some_and(|inner| inner.is::<UploadTooLarge>())
    }) {
        return StatusCode::PAYLOAD_TOO_LARGE;
    }
    if err.downcast_ref::<BlobRejected>().is_some() {
        return StatusCode::BAD_REQUEST;
    }
    StatusCode::INTERNAL_SERVER_ERROR
}

// Clients send `?hash=` as 64 hex, like every other `Hash` on the wire; only the
// digest bytes carry over into the `ContentHash` that `add_blob` compares against.
fn parse_expected_content_hash(hash_str: &str) -> Option<ContentHash> {
    let hash: Hash = hash_str.parse().ok()?;
    Some(ContentHash::from(*hash.as_bytes()))
}

/// The context an account-scoped blob request is confined to.
///
/// A blob carries no owner, so an account is never scoped to "its" blobs; it
/// is scoped to its CONTEXTS. Every account-scoped upload and read therefore
/// names a `context_id`, and that context must be one whose group the account
/// is a member of — resolved per request through the same `caller_scope`
/// predicate the context reads use, so a removed member stops being served
/// when the governance op lands, not when the session expires.
///
/// `Ok(None)` is the node-wide caller (a node owner, or a node with no auth
/// guard), which is served exactly as before. `Err` is the refusal to send.
fn account_context(
    state: &AdminState,
    scope: &ListScope,
    context_id: Option<ContextId>,
) -> Result<Option<ContextId>, ApiError> {
    if matches!(scope, ListScope::NodeWide) {
        return Ok(None);
    }

    let Some(context_id) = context_id else {
        return Err(ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "an account-scoped blob request must name the context_id it is for".to_owned(),
        });
    };

    match admits_context(state.ctx_client.datastore(), &context_id, scope) {
        Ok(true) => Ok(Some(context_id)),
        Ok(false) => {
            info!(%context_id, "Refusing blob request: caller is not a member of this context's group");
            Err(ApiError {
                status_code: StatusCode::FORBIDDEN,
                message: "account is not a member of the group owning this context".to_owned(),
            })
        }
        Err(err) => {
            error!(%context_id, error=?err, "Failed to resolve the context's group");
            Err(parse_api_error(err))
        }
    }
}

/// The answer an account-scoped caller gets for a blob it may not read through
/// the context it named.
///
/// Deliberately the same `404` as a blob nobody holds. A distinct refusal would
/// be an oracle: a member of context A could learn whether this node holds a
/// blob of context B by naming its id.
fn blob_not_found() -> Response {
    ApiError {
        status_code: StatusCode::NOT_FOUND,
        message: "Blob not found locally or in network".to_owned(),
    }
    .into_response()
}

/// Whether an account-scoped caller confined to `context_id` may be served
/// `blob_id` WITHOUT a network fetch: this node associates the two.
///
/// `Ok(Some(true))` — associated, serve it. `Ok(Some(false))` — this node holds
/// the bytes but not for this context: refuse, and do not go to the network,
/// which would only return the local copy. `Ok(None)` — not held here at all,
/// so a fetch from the context's peers may establish the association; the
/// caller must re-check it afterwards with
/// [`NodeClient::is_blob_in_context`](calimero_node_primitives::client::NodeClient::is_blob_in_context),
/// because the bytes could land locally for another context in between.
fn local_association(
    state: &AdminState,
    blob_id: &BlobId,
    context_id: &ContextId,
) -> eyre::Result<Option<bool>> {
    if state.node_client.is_blob_in_context(blob_id, context_id)? {
        return Ok(Some(true));
    }
    if state.node_client.has_blob(blob_id)? {
        return Ok(Some(false));
    }
    Ok(None)
}

/// Upload a blob via raw binary data (streaming version)
///
/// This endpoint accepts raw binary data in the request body and streams it
/// directly to blob storage without loading it all into memory first.
/// Perfect for large file uploads with minimal memory usage.
///
/// Query parameters:
/// - `hash`: Expected hash of the blob for verification (optional)
/// - `context_id`: Context the blob is for: announced to its peers for network
///   discovery, and recorded as the blob's context on this node. Optional for a
///   node-wide caller; **required** for an account-scoped one, which must be a
///   member of the context's group and is held to
///   [`MAX_ACCOUNT_BLOB_UPLOAD_BYTES`]. Peers are served only blobs uploaded
///   for one of this node's contexts.
pub async fn upload_handler(
    Query(query): Query<BlobUploadQuery>,
    Extension(state): Extension<Arc<AdminState>>,
    node_owner: Option<Extension<AuthenticatedNodeOwner>>,
    account: Option<Extension<AuthenticatedAccount>>,
    device: Option<Extension<AuthenticatedDevice>>,
    body: Body,
) -> impl IntoResponse {
    let expected_hash = if let Some(hash_str) = query.hash {
        match parse_expected_content_hash(&hash_str) {
            Some(hash) => Some(hash),
            None => {
                return ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "The provided hash is not a valid format".to_owned(),
                }
                .into_response();
            }
        }
    } else {
        None
    };

    let context_id = if let Some(context_id_str) = query.context_id {
        match context_id_str.parse() {
            Ok(context_id) => {
                info!("Uploading blob with context announcement");
                debug!(context_id=%context_id, "Will announce blob to context");
                Some(context_id)
            }
            Err(_) => {
                return ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "The provided context_id is not a valid format".to_owned(),
                }
                .into_response();
            }
        }
    } else {
        None
    };

    // Decided before a byte of the body is read: a refused caller must not get
    // to fill the disk first.
    let scope = match list_scope_for(&state.ctx_client, node_owner, account, device) {
        Ok(scope) => scope,
        Err(err) => {
            error!(error=?err, "Failed to resolve the caller's list scope");
            return parse_api_error(err).into_response();
        }
    };
    let confined_to = match account_context(&state, &scope, context_id) {
        Ok(confined_to) => confined_to,
        Err(refusal) => return refusal.into_response(),
    };
    let limit = if confined_to.is_some() {
        MAX_ACCOUNT_BLOB_UPLOAD_BYTES
    } else {
        MAX_BLOB_UPLOAD_BYTES
    };

    info!("Uploading blob");
    debug!(has_expected_hash=%expected_hash.is_some(), has_context=%context_id.is_some(), "Blob upload request");

    let reader = body_to_async_read(body, limit);

    match state
        .node_client
        .add_blob(reader, None, expected_hash.as_ref())
        .await
    {
        Ok((blob_id, size)) => {
            info!(blob_id=%blob_id, "Blob uploaded successfully");
            debug!(
                blob_id=%blob_id,
                size_bytes=%size,
                size_mib=%(size as f64 / (1024.0 * 1024.0)),
                "Blob upload details"
            );

            // The caller put these bytes here for this context, so they are the
            // context's on this node: what lets its account-scoped members read
            // them back (`Column::ContextBlob`). Recorded for a node-wide
            // caller too, so an operator's upload is readable by the context's
            // delegated members. An account's upload that cannot be recorded is
            // one it could never read back, so that is its failure; for a
            // node-wide caller nothing it can do depends on the row.
            if let Some(ctx_id) = context_id {
                if let Err(err) = state.node_client.record_blob_context(&blob_id, &ctx_id) {
                    error!(blob_id=%blob_id, context_id=%ctx_id, error=?err, "Failed to record the blob's context");
                    if confined_to.is_some() {
                        return parse_api_error(err).into_response();
                    }
                }
            }

            // Announce blob to network if context_id is provided.
            //
            // Announcing writes a discovery record advertising that this node
            // holds the blob for the given context, and recording the blob for
            // the context is what lets its peers be served it. Do both only for
            // a context this node actually participates in: without this check a
            // caller could inject blob-availability records into arbitrary
            // contexts' discovery. Membership is proven by the node owning an
            // identity in the context (the same signal the signed serving path
            // relies on). A context this node is not a member of still gets the
            // upload (an account-scoped caller reads it back through
            // `Column::ContextBlob`), but its peers are never served the blob.
            if let Some(ctx_id) = context_id {
                match state.node_client.find_owned_identity(&ctx_id) {
                    Ok(Some(_)) => {
                        if let Err(err) = state.node_client.record_blob_owner(&ctx_id, &blob_id) {
                            error!(blob_id=%blob_id, context_id=%ctx_id, error=?err, "Failed to record the blob for its context");
                            return parse_api_error(err).into_response();
                        }
                        match state
                            .node_client
                            .announce_blob_to_network(&blob_id, &ctx_id, size)
                            .await
                        {
                            Ok(_) => {
                                info!(blob_id=%blob_id, context_id=%ctx_id, "Blob announced to network");
                            }
                            Err(err) => {
                                error!(blob_id=%blob_id, context_id=%ctx_id, error=?err, "Failed to announce blob to network");
                            }
                        }
                    }
                    Ok(None) => {
                        debug!(blob_id=%blob_id, context_id=%ctx_id, "Not a member of the context; blob not recorded for its peers or announced");
                    }
                    Err(err) => {
                        error!(blob_id=%blob_id, context_id=%ctx_id, error=?err, "Failed to verify context membership; blob not announced");
                    }
                }
            }

            ApiResponse {
                payload: BlobUploadResponse {
                    data: BlobInfo { blob_id, size },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(error=?err, "Failed to upload blob");
            ApiError {
                status_code: upload_refusal_status(&err),
                message: format!("Failed to store blob: {err}"),
            }
            .into_response()
        }
    }
}

/// List all blobs
///
/// Returns a list of all root blob IDs and their metadata. Root blobs are either:
/// - Blobs that contain links to chunks (segmented large files)
/// - Standalone blobs that aren't referenced as chunks by other blobs
///
/// This excludes individual chunk blobs to provide a cleaner user experience.
pub async fn list_handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    info!("Listing blobs");

    match state.node_client.list_blobs() {
        Ok(blobs) => {
            info!(count=%blobs.len(), "Blobs listed successfully");
            debug!(blob_ids=?blobs.iter().map(|b| b.blob_id).collect::<Vec<_>>(), "Blob list");
            ApiResponse {
                payload: BlobListResponse {
                    data: BlobListResponseData { blobs },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(error=?err, "Failed to list blobs");
            parse_api_error(err).into_response()
        }
    }
}

/// Names where a blob lookup got its answer, in `X-Blob-Source`.
///
/// A caller must be able to tell "this node holds it" from "some peer says it
/// does" without inferring it from which headers happen to be missing: the
/// second answer carries a size and nothing else, because `X-Blob-Hash` and
/// `X-Blob-MIME-Type` are derived from bytes this node does not have. An
/// explicit header says which answer this is; a missing `X-Blob-Hash` would
/// otherwise be indistinguishable from a bug.
const BLOB_SOURCE_HEADER: &str = "X-Blob-Source";

/// `X-Blob-Source` value: served from (or verified against) this node's store.
const BLOB_SOURCE_LOCAL: &str = "local";

/// `X-Blob-Source` value: a context peer answered a probe. Presence and size
/// only, and neither is verified — verifying either means transferring the
/// blob, which `HEAD` deliberately never does.
const BLOB_SOURCE_PEER: &str = "peer";

/// The `X-Blob-*` headers a client may read cross-origin.
///
/// One list, used by both the full-metadata and the peer-presence response, so
/// the two cannot drift.
const EXPOSED_BLOB_HEADERS: &str = "X-Blob-ID, X-Blob-Hash, X-Blob-MIME-Type, X-Blob-Source, ETag";

/// Helper function to build response headers from blob metadata
fn build_blob_response_headers(blob_metadata: &BlobMetadata, blob_id: BlobId) -> Builder {
    let etag = format!("\"{}\"", hex::encode(blob_metadata.hash));

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Length", blob_metadata.size.to_string())
        .header("Content-Type", &blob_metadata.mime_type)
        .header("ETag", &etag)
        // Blobs may be private context data served through this admin API, so
        // never allow shared proxies/CDNs to cache them (`private`). We use
        // `no-cache` (revalidate before reuse) rather than `immutable`: a blob
        // id can be deleted via this same API, and `immutable` would let a
        // client keep serving a since-deleted blob for the max-age window.
        // Revalidation is cheap for an admin API, so correctness wins.
        .header("Cache-Control", "private, no-cache")
        .header("X-Blob-ID", blob_id.to_string())
        .header("X-Blob-Hash", hex::encode(blob_metadata.hash))
        .header("X-Blob-MIME-Type", &blob_metadata.mime_type)
        // Every response built here is backed by bytes in this node's store —
        // `download_handler` reaches it only after discovery has stored the
        // blob locally — so the hash and MIME type above are ours, not a
        // peer's claim.
        .header(BLOB_SOURCE_HEADER, BLOB_SOURCE_LOCAL)
        .header("Access-Control-Expose-Headers", EXPOSED_BLOB_HEADERS)
        .header("X-Content-Type-Options", "nosniff")
        .header("Content-Disposition", "attachment")
        .header("Content-Security-Policy", "sandbox; default-src 'none'")
}

/// Headers for a blob this node does not hold but a context peer answered for.
///
/// Deliberately narrower than [`build_blob_response_headers`]: no `ETag`, no
/// `Content-Type`, no `X-Blob-Hash`, no `X-Blob-MIME-Type`. All four are
/// computed from the bytes — the hash from `BlobMeta`, the MIME type sniffed
/// from the first chunk — and this node has none. Emitting a placeholder would
/// hand the client a hash nobody computed and let a cache key on it, so the
/// headers are omitted and `X-Blob-Source: peer` says why.
///
/// `Content-Length` carries the size the holder reported, and is omitted when
/// it reported none: a `HEAD` with no `Content-Length` says "exists, size
/// unknown", which is true, where `0` would be a lie about an existing blob.
fn build_peer_presence_headers(blob_id: BlobId, size: Option<u64>) -> Builder {
    let builder = Response::builder()
        .status(StatusCode::OK)
        // Same reasoning as the local path: never shared-cacheable, always
        // revalidated. More so here — the answer is one peer's word.
        .header("Cache-Control", "private, no-cache")
        .header("X-Blob-ID", blob_id.to_string())
        .header(BLOB_SOURCE_HEADER, BLOB_SOURCE_PEER)
        .header("Access-Control-Expose-Headers", EXPOSED_BLOB_HEADERS);

    match size {
        Some(size) => builder.header("Content-Length", size.to_string()),
        None => builder,
    }
}

/// Download a blob by its ID
///
/// Returns the raw binary data of the blob with complete metadata headers.
/// Headers are identical to HEAD request for the same blob.
///
/// An account-scoped caller must name a `context_id` it is a member of, and is
/// served only a blob this node associates with that context — one uploaded
/// for it, or fetched from its peers (which this request may do, when the blob
/// is not held here at all). Anything else is the same `404` as a missing blob.
pub async fn download_handler(
    Path(blob_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    Extension(state): Extension<Arc<AdminState>>,
    node_owner: Option<Extension<AuthenticatedNodeOwner>>,
    account: Option<Extension<AuthenticatedAccount>>,
    device: Option<Extension<AuthenticatedDevice>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let blob_id: BlobId = match blob_id.parse() {
        Ok(id) => id,
        Err(err) => {
            error!(blob_id=%blob_id, error=?err, "Invalid blob ID format");
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid blob ID format".to_owned(),
            }
            .into_response();
        }
    };

    // Get blob with optional network discovery
    let context_id = if let Some(context_id_str) = params.get("context_id") {
        // Parse context_id
        match context_id_str.parse() {
            Ok(context_id) => {
                info!(blob_id=%blob_id, context_id=%context_id, "Downloading blob with network discovery");
                debug!(blob_id=%blob_id, context_id=%context_id, "Blob download request");
                Some(context_id)
            }
            Err(err) => {
                error!(context_id=%context_id_str, error=?err, "Invalid context ID format");
                return ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "Invalid context ID format".to_owned(),
                }
                .into_response();
            }
        }
    } else {
        info!(blob_id=%blob_id, "Downloading blob");
        debug!(blob_id=%blob_id, "Blob download request");
        None
    };

    let scope = match list_scope_for(&state.ctx_client, node_owner, account, device) {
        Ok(scope) => scope,
        Err(err) => {
            error!(%blob_id, error=?err, "Failed to resolve the caller's list scope");
            return parse_api_error(err).into_response();
        }
    };
    let confined_to = match account_context(&state, &scope, context_id) {
        Ok(confined_to) => confined_to,
        Err(refusal) => return refusal.into_response(),
    };
    // `true` when a fetch must establish the association before anything is
    // served: the blob is not held here at all.
    let needs_fetched_association = match confined_to {
        None => false,
        Some(ctx_id) => match local_association(&state, &blob_id, &ctx_id) {
            Ok(Some(true)) => false,
            Ok(Some(false)) => return blob_not_found(),
            Ok(None) => true,
            Err(err) => {
                error!(%blob_id, context_id=%ctx_id, error=?err, "Failed to read the blob's context association");
                return parse_api_error(err).into_response();
            }
        },
    };

    let blob_result = state
        .node_client
        .get_blob(&blob_id, peers_of(context_id, &headers).as_ref())
        .await;

    // `get_blob` answers from the local store first, so a copy that landed
    // for another context since the check above would come back here. Only a
    // fetch from this context's peers records the association, so re-reading
    // it is what tells the two apart.
    if let (Some(ctx_id), true, Ok(Some(_))) =
        (confined_to, needs_fetched_association, &blob_result)
    {
        match state.node_client.is_blob_in_context(&blob_id, &ctx_id) {
            Ok(true) => {}
            Ok(false) => return blob_not_found(),
            Err(err) => {
                error!(%blob_id, context_id=%ctx_id, error=?err, "Failed to read the blob's context association");
                return parse_api_error(err).into_response();
            }
        }
    }

    match blob_result {
        Ok(Some(blob)) => {
            // Now get metadata for headers (blob should be local after discovery).
            //
            // The metadata drives the Content-Length and ETag response headers. If
            // it is missing or unreadable we must NOT fabricate zero metadata: that
            // would emit `Content-Length: 0` and an all-zero ETag while streaming a
            // non-empty body, causing clients to truncate the response and caches to
            // collide distinct blobs under the same `"00..00"` ETag. A blob whose
            // bytes exist but whose metadata is gone is an inconsistent store state,
            // so surface it as a 500 rather than serving a corrupt response.
            let blob_metadata = match state.node_client.get_blob_info(blob_id).await {
                Ok(Some(metadata)) => metadata,
                Ok(None) => {
                    error!(%blob_id, "Blob bytes found but metadata missing; refusing to serve");
                    return ApiError {
                        status_code: StatusCode::INTERNAL_SERVER_ERROR,
                        message: "Blob metadata is unavailable".to_owned(),
                    }
                    .into_response();
                }
                Err(err) => {
                    error!(%blob_id, ?err, "Failed to read blob metadata; refusing to serve");
                    return ApiError {
                        status_code: StatusCode::INTERNAL_SERVER_ERROR,
                        message: "Failed to read blob metadata".to_owned(),
                    }
                    .into_response();
                }
            };

            tracing::debug!(
                "Serving blob {} via streaming with metadata headers",
                blob_id
            );

            let stream = blob.map(|result| result.map_err(std::io::Error::other));

            build_blob_response_headers(&blob_metadata, blob_id)
                .body(Body::from_stream(stream))
                .unwrap_or_else(|_| {
                    ApiError {
                        status_code: StatusCode::INTERNAL_SERVER_ERROR,
                        message: "Failed to build response".to_owned(),
                    }
                    .into_response()
                })
        }
        Ok(None) => ApiError {
            status_code: StatusCode::NOT_FOUND,
            message: "Blob not found locally or in network".to_owned(),
        }
        .into_response(),
        Err(err) => {
            tracing::error!("Failed to retrieve blob {}: {:?}", blob_id, err);
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to retrieve blob: {err}"),
            }
            .into_response()
        }
    }
}

/// Delete a blob by its ID
///
/// Removes blob metadata from database and deletes the actual blob files.
/// This includes all associated chunk files for large blobs.
pub async fn delete_handler(
    Path(blob_id): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let blob_id: BlobId = match blob_id.parse() {
        Ok(id) => id,
        Err(_) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid blob ID format".to_owned(),
            }
            .into_response();
        }
    };

    tracing::info!("Attempting to delete blob {}", blob_id);

    // Blob deletion is a global, reference-counted operation with no per-caller
    // ownership: any authenticated caller can release a reference to any blob by
    // its global id. That is acceptable for ordinary content, but application
    // bytecode/compiled artifacts are shared blobs that installed apps depend on
    // to execute. Releasing the last reference to one would brick every context
    // running that app. Refuse to delete blobs that are referenced as an
    // application artifact.
    match state.node_client.is_blob_application_artifact(&blob_id) {
        Ok(true) => {
            tracing::warn!(%blob_id, "refusing to delete blob referenced by an installed application");
            return ApiError {
                status_code: StatusCode::FORBIDDEN,
                message: "Blob is referenced by an installed application and cannot be deleted"
                    .to_owned(),
            }
            .into_response();
        }
        Ok(false) => {}
        Err(err) => {
            tracing::error!(%blob_id, ?err, "failed to check application references before delete");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to verify blob is safe to delete".to_owned(),
            }
            .into_response();
        }
    }

    match state.node_client.delete_blob(blob_id).await {
        Ok(true) => {
            tracing::info!("Successfully deleted blob {}", blob_id);
            ApiResponse {
                payload: BlobDeleteResponse {
                    blob_id,
                    deleted: true,
                },
            }
            .into_response()
        }
        Ok(false) => {
            tracing::warn!("Blob {} not found or already deleted", blob_id);
            ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Blob not found".to_owned(),
            }
            .into_response()
        }
        Err(err) => {
            tracing::error!("Failed to delete blob {}: {:?}", blob_id, err);
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to delete blob: {err}"),
            }
            .into_response()
        }
    }
}

/// Get blob metadata via HEAD request
///
/// Returns blob metadata in HTTP headers without the actual blob content.
/// This is efficient for checking blob existence and getting size info.
/// Also detects and returns MIME type based on file content.
///
/// # Optional network discovery
///
/// With `?context_id=<id>` — the same opt-in `download_handler` takes — a blob
/// this node does not hold is looked for among the context's peers by probing
/// them, which answers presence and size without transferring a byte. **`HEAD`
/// never transfers the blob, with or without a context.**
///
/// Discovery is opt-in and not the default because a probe sweep can run to the
/// node client's 30s discovery deadline, while `HEAD` reads as a cheap call.
/// Without the parameter this endpoint is exactly what it was: one local store
/// read.
///
/// # Response headers
///
/// `X-Blob-Source` names where the answer came from, and decides which of the
/// other headers are present:
///
/// - `local` — this node holds the blob. `Content-Length`, `Content-Type`,
///   `ETag`, `X-Blob-Hash` and `X-Blob-MIME-Type` are all present, exactly as
///   before this parameter existed.
/// - `peer` — only a context peer holds it. `Content-Length` carries the size
///   it reported; `ETag`, `Content-Type`, `X-Blob-Hash` and `X-Blob-MIME-Type`
///   are **absent**, because all of them are derived from bytes this node does
///   not have and are not worth a download to fabricate.
///
/// A blob held neither locally nor by any probed peer is a 404, as before.
///
/// # Account-scoped callers
///
/// Confined as `download_handler` confines them: a `context_id` is required
/// and must be one of the caller's. A blob held here is described only when
/// this node associates it with that context; one not held here may still be
/// answered `peer`, which tells a member only that a peer of its own context
/// holds it — what its own node would learn by probing. A `HEAD` never fetches,
/// so it never creates an association.
pub async fn info_handler(
    Path(blob_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    Extension(state): Extension<Arc<AdminState>>,
    node_owner: Option<Extension<AuthenticatedNodeOwner>>,
    account: Option<Extension<AuthenticatedAccount>>,
    device: Option<Extension<AuthenticatedDevice>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let blob_id: BlobId = match blob_id.parse() {
        Ok(id) => id,
        Err(_) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid blob ID format".to_owned(),
            }
            .into_response();
        }
    };

    let context_id = match params.get("context_id").map(|raw| raw.parse()).transpose() {
        Ok(context_id) => context_id,
        Err(err) => {
            error!(?err, "Invalid context ID format");
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid context ID format".to_owned(),
            }
            .into_response();
        }
    };

    let scope = match list_scope_for(&state.ctx_client, node_owner, account, device) {
        Ok(scope) => scope,
        Err(err) => {
            error!(%blob_id, error=?err, "Failed to resolve the caller's list scope");
            return parse_api_error(err).into_response();
        }
    };
    let confined_to = match account_context(&state, &scope, context_id) {
        Ok(confined_to) => confined_to,
        Err(refusal) => return refusal.into_response(),
    };
    // Whether a local answer may be given. `false` only for an account-scoped
    // caller whose context this node does not associate the blob with.
    let may_describe_local = match confined_to {
        None => true,
        Some(ctx_id) => match local_association(&state, &blob_id, &ctx_id) {
            Ok(Some(true)) => true,
            Ok(Some(false)) => return blob_not_found(),
            Ok(None) => false,
            Err(err) => {
                error!(%blob_id, context_id=%ctx_id, error=?err, "Failed to read the blob's context association");
                return parse_api_error(err).into_response();
            }
        },
    };

    let presence = state
        .node_client
        .get_blob_presence(blob_id, peers_of(context_id, &headers).as_ref())
        .await;

    let headers = match presence {
        // A local copy that is not this context's — including one that landed
        // since the check above — is not described to an account caller.
        Ok(Some(BlobPresence::Local(_))) if !may_describe_local => return blob_not_found(),
        Ok(Some(BlobPresence::Local(blob_metadata))) => {
            build_blob_response_headers(&blob_metadata, blob_id)
        }
        Ok(Some(BlobPresence::Peer { size, .. })) => build_peer_presence_headers(blob_id, size),
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Blob not found".to_owned(),
            }
            .into_response()
        }
        Err(err) => return parse_api_error(err).into_response(),
    };

    headers.body(Body::empty()).unwrap_or_else(|_| {
        ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Failed to build response".to_owned(),
        }
        .into_response()
    })
}

#[cfg(test)]
mod upload_refusal_status_tests {
    use std::sync::Arc;

    use calimero_primitives::content_hash::ContentHash;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use futures_util::io::Cursor;
    use tokio::sync::broadcast;

    use super::*;

    async fn a_node_client() -> (
        calimero_node_primitives::client::NodeClient,
        tempfile::TempDir,
    ) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (event_sender, _events) = broadcast::channel(8);
        crate::test_support::test_node_client(&store, LazyRecipient::new(), event_sender).await
    }

    /// The upload stream's size-limit refusal survives the blob store and
    /// reads as `413`, not the generic `500`.
    #[tokio::test]
    async fn an_upload_over_the_limit_answers_413() {
        let (node_client, _blob_dir) = a_node_client().await;
        let over_the_limit = futures_util::stream::iter([Err::<axum::body::Bytes, _>(
            std::io::Error::new(std::io::ErrorKind::InvalidData, UploadTooLarge),
        )]);
        let reader = StreamReader::new(over_the_limit).compat();

        let err = node_client
            .add_blob(reader, None, None)
            .await
            .expect_err("the stream refused the body");
        assert_eq!(
            upload_refusal_status(&err),
            StatusCode::PAYLOAD_TOO_LARGE,
            "got: {err:#}"
        );
    }

    /// Bytes that do not hash to the caller's `?hash=` are the caller's
    /// mistake: `400`.
    #[tokio::test]
    async fn an_upload_that_misses_its_hash_answers_400() {
        let (node_client, _blob_dir) = a_node_client().await;

        let err = node_client
            .add_blob(
                Cursor::new(b"some bytes"),
                None,
                Some(&ContentHash::from([0x5A; 32])),
            )
            .await
            .expect_err("the hash does not match");
        assert_eq!(
            upload_refusal_status(&err),
            StatusCode::BAD_REQUEST,
            "got: {err:#}"
        );
    }

    /// Anything else is still a storage fault.
    #[test]
    fn any_other_failure_stays_500() {
        assert_eq!(
            upload_refusal_status(&eyre::eyre!("disk full")),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}

#[cfg(test)]
mod upload_sharing_tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::extract::Query;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::Extension;
    use calimero_context_client::client::ContextClient;
    use calimero_node_primitives::client::NodeClient;
    use calimero_node_primitives::test_fixtures;
    use calimero_primitives::blobs::BlobId;
    use calimero_primitives::context::ContextId;
    use calimero_store::db::InMemoryDB;
    use calimero_store::{key, types, Store};
    use calimero_utils_actix::LazyRecipient;

    use super::{upload_handler, BlobUploadQuery};
    use crate::auth::AuthenticatedNodeOwner;
    use crate::{AdminState, NodeReadiness};

    const MEMBER_OF: [u8; 32] = [0xA1; 32];

    /// A fresh node that owns an identity in `MEMBER_OF` and nowhere else.
    async fn node() -> (
        Arc<AdminState>,
        NodeClient,
        tempfile::TempDir,
        tempfile::TempDir,
    ) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        store
            .handle()
            .put(
                &key::ContextIdentity::new(MEMBER_OF.into(), [0x0A; 32].into()),
                &types::ContextIdentity {
                    private_key: Some([0x0B; 32]),
                },
            )
            .unwrap();
        let (node_client, data, blobs) = test_fixtures::node_client_over(
            store.clone(),
            test_fixtures::network_accepting_announces(),
        )
        .await;
        let ctx_client =
            ContextClient::new(store.clone(), node_client.clone(), LazyRecipient::new());
        let state = Arc::new(AdminState::new(
            store,
            ctx_client,
            node_client.clone(),
            Arc::new(NodeReadiness::new()),
            [0; 32],
            #[cfg(feature = "mock-attestation")]
            false,
        ));
        (state, node_client, data, blobs)
    }

    async fn upload_status(
        state: &Arc<AdminState>,
        bytes: &'static [u8],
        context: Option<ContextId>,
    ) -> StatusCode {
        let query = BlobUploadQuery {
            hash: None,
            context_id: context.map(|context| context.to_string()),
        };
        upload_handler(
            Query(query),
            Extension(Arc::clone(state)),
            Some(Extension(AuthenticatedNodeOwner)),
            None,
            None,
            Body::from(bytes),
        )
        .await
        .into_response()
        .status()
    }

    async fn upload(
        state: &Arc<AdminState>,
        bytes: &'static [u8],
        context: Option<ContextId>,
    ) -> BlobId {
        assert_eq!(upload_status(state, bytes, context).await, StatusCode::OK);
        let blobs = state.node_client.list_blobs().unwrap();
        assert_eq!(blobs.len(), 1, "each case runs on a fresh node");
        blobs[0].blob_id
    }

    #[actix::test]
    async fn an_upload_is_shared_only_in_a_context_this_node_is_in() {
        let member_of = ContextId::from(MEMBER_OF);

        let (state, node_client, _data, _blobs) = node().await;
        let shared = upload(&state, b"uploaded for a context we are in", Some(member_of)).await;
        assert!(node_client
            .is_blob_held_for_context(&member_of, &shared)
            .unwrap());

        let (state, node_client, _data, _blobs) = node().await;
        let unowned = upload(&state, b"uploaded with no context", None).await;
        assert!(!node_client
            .is_blob_held_for_context(&member_of, &unowned)
            .unwrap());
    }

    /// The upload is kept (an account-scoped caller reads it back through its
    /// context), but this node is not in the context, so its peers are never
    /// served the blob.
    #[actix::test]
    async fn an_upload_for_a_context_this_node_is_not_in_is_not_shared_with_its_peers() {
        let not_a_member = ContextId::from([0xB1; 32]);
        let (state, node_client, _data, _blobs) = node().await;

        let blob = upload(
            &state,
            b"uploaded for a context we are not in",
            Some(not_a_member),
        )
        .await;

        assert!(!node_client
            .is_blob_held_for_context(&not_a_member, &blob)
            .unwrap());
    }
}

#[cfg(test)]
mod parse_expected_content_hash_tests {
    use super::*;

    /// `?hash=` takes hex, and base58 is now refused outright.
    ///
    /// This inverts a pin that read `base58_is_accepted_hex_is_rejected`, kept
    /// that way because "out-of-repo clients already send it that way". They do,
    /// and this is a break for them — a deliberate one: the parser is
    /// `Hash::from_str`, so `?hash=` could not stay base58 without the type
    /// carrying a second parser purely for one query string.
    ///
    /// Refusing base58 rather than accepting both is what makes it a break the
    /// caller *sees*. Most base58 hashes are not valid hex, so they fail here; but
    /// hex digits are a strict subset of the base58 alphabet, so a permissive
    /// parser would decode some strings under the wrong alphabet and verify the
    /// upload against a hash nobody asked for.
    #[test]
    fn hex_is_accepted_base58_is_rejected() {
        let bytes = [0xAB_u8; 32];
        let hex = ContentHash::from(bytes).to_string();

        assert_eq!(
            parse_expected_content_hash(&hex),
            Some(ContentHash::from(bytes))
        );
        // 32 bytes of 0xAB in base58 — no longer a hash this endpoint knows.
        assert_eq!(
            parse_expected_content_hash("CZ8YUVdk7znjrUmnb5n7kgySk9yRAsQDYmyCxzfSky9t"),
            None
        );
    }
}

#[cfg(test)]
mod local_blob_header_tests {
    use super::build_blob_response_headers;
    use calimero_primitives::blobs::{BlobId, BlobMetadata};

    #[test]
    fn a_blob_never_renders_as_a_page_on_the_node_origin() {
        let blob_id = BlobId::from([7; 32]);
        let metadata = BlobMetadata {
            blob_id,
            size: 21,
            hash: [9; 32],
            mime_type: "text/html".to_owned(),
        };
        let response = build_blob_response_headers(&metadata, blob_id)
            .body(())
            .expect("headers to build");
        let headers = response.headers();

        assert_eq!(headers["X-Content-Type-Options"], "nosniff");
        assert_eq!(headers["Content-Disposition"], "attachment");
        let csp = headers["Content-Security-Policy"].to_str().unwrap();
        assert!(csp.contains("sandbox"), "{csp}");
        assert!(csp.contains("default-src 'none'"), "{csp}");
        assert_eq!(headers["Content-Type"], "text/html");
    }
}

#[cfg(test)]
mod peer_presence_header_tests {
    use super::{
        build_peer_presence_headers, BLOB_SOURCE_HEADER, BLOB_SOURCE_PEER, EXPOSED_BLOB_HEADERS,
    };
    use calimero_primitives::blobs::BlobId;

    /// The peer answer must not carry anything derived from bytes this node
    /// does not have. A regression here would publish a hash nobody computed —
    /// and let a cache key on it.
    #[test]
    fn a_peer_answer_omits_the_locally_derived_headers() {
        let blob_id = BlobId::from([3; 32]);
        let response = build_peer_presence_headers(blob_id, Some(1024))
            .body(())
            .expect("headers to build");
        let headers = response.headers();

        assert_eq!(headers["Content-Length"], "1024");
        assert_eq!(headers[BLOB_SOURCE_HEADER], BLOB_SOURCE_PEER);
        assert_eq!(headers["X-Blob-ID"], blob_id.to_string());
        assert_eq!(
            headers["Access-Control-Expose-Headers"],
            EXPOSED_BLOB_HEADERS
        );

        for absent in ["ETag", "Content-Type", "X-Blob-Hash", "X-Blob-MIME-Type"] {
            assert!(
                !headers.contains_key(absent),
                "{absent} is derived from the blob's bytes, which this node does not hold"
            );
        }
    }

    /// "Exists, size unknown" is a true answer; `Content-Length: 0` about a
    /// blob that exists is not.
    #[test]
    fn an_unreported_size_omits_content_length() {
        let response = build_peer_presence_headers(BlobId::from([3; 32]), None)
            .body(())
            .expect("headers to build");

        assert!(!response.headers().contains_key("Content-Length"));
        assert_eq!(response.headers()[BLOB_SOURCE_HEADER], BLOB_SOURCE_PEER);
    }
}

#[cfg(test)]
mod account_scope_tests {
    //! The blob routes as an account-scoped caller (a delegated session on a
    //! relay) meets them, against a real store and blob store.
    //!
    //! The guard's half — which sessions reach these handlers at all, and that
    //! an anonymous caller does not — is pinned in `crate::auth`'s tests.

    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::extract::{Path, Query};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::Extension;
    use calimero_account::AccountId;
    use calimero_context_client::client::ContextClient;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{ContextTreeService, MembershipRepository};
    use calimero_primitives::context::{ContextId, GroupMemberRole};
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use tokio::sync::broadcast;

    use super::{
        download_handler, info_handler, upload_handler, BlobUploadQuery,
        MAX_ACCOUNT_BLOB_UPLOAD_BYTES,
    };
    use crate::auth::{AuthenticatedAccount, AuthenticatedNodeOwner};
    use crate::{AdminState, NodeReadiness};

    const ME: [u8; 32] = [0x01; 32];
    const THEM: [u8; 32] = [0x02; 32];

    fn my_context() -> ContextId {
        ContextId::from([0x11; 32])
    }

    fn their_context() -> ContextId {
        ContextId::from([0x22; 32])
    }

    /// Who is calling, as the extensions the guard (or `proxy_identity`)
    /// injects say.
    #[derive(Clone, Copy)]
    enum Caller {
        /// An account-anchored session: narrowed to the account's groups.
        Account([u8; 32]),
        /// The node's owner: the node-wide view.
        Owner,
        /// No identity at all — `AuthMode::Proxy` without `proxy_identity`.
        Unguarded,
    }

    impl Caller {
        fn owner(self) -> Option<Extension<AuthenticatedNodeOwner>> {
            matches!(self, Self::Owner).then_some(Extension(AuthenticatedNodeOwner))
        }

        fn account(self) -> Option<Extension<AuthenticatedAccount>> {
            match self {
                Self::Account(id) => Some(Extension(AuthenticatedAccount(AccountId::from(id)))),
                Self::Owner | Self::Unguarded => None,
            }
        }
    }

    /// A node holding two tenants' contexts: mine in my group, theirs in theirs.
    struct Node {
        state: Arc<AdminState>,
        _blob_dir: tempfile::TempDir,
    }

    async fn node() -> Node {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (event_sender, _events) = broadcast::channel(8);
        let (node_client, blob_dir) =
            crate::test_support::test_node_client(&store, LazyRecipient::new(), event_sender).await;
        let ctx_client =
            ContextClient::new(store.clone(), node_client.clone(), LazyRecipient::new());

        for (group, context, member) in [
            ([0xa1; 32], my_context(), ME),
            ([0xb1; 32], their_context(), THEM),
        ] {
            let group = ContextGroupId::from(group);
            ContextTreeService::new(&store, group)
                .register_context(&context)
                .expect("register context");
            MembershipRepository::new(&store)
                .add_member(&group, &AccountId::from(member), GroupMemberRole::Member)
                .expect("add member");
        }

        Node {
            state: Arc::new(AdminState {
                store,
                ctx_client,
                node_client,
                readiness: Arc::new(NodeReadiness::new()),
                transport_public_key: [0; 32],
                #[cfg(feature = "mock-attestation")]
                mock_tee: false,
                tee_release_version: None,
            }),
            _blob_dir: blob_dir,
        }
    }

    fn context_query(context: Option<ContextId>) -> std::collections::HashMap<String, String> {
        context
            .map(|c| ("context_id".to_owned(), c.to_string()))
            .into_iter()
            .collect()
    }

    async fn upload(
        node: &Node,
        caller: Caller,
        context: Option<ContextId>,
        body: Body,
    ) -> Response {
        upload_handler(
            Query(BlobUploadQuery {
                hash: None,
                context_id: context.map(|c| c.to_string()),
            }),
            Extension(Arc::clone(&node.state)),
            caller.owner(),
            caller.account(),
            None,
            body,
        )
        .await
        .into_response()
    }

    /// Upload `bytes` and return the blob id the node answered with.
    async fn uploaded(
        node: &Node,
        caller: Caller,
        context: Option<ContextId>,
        bytes: &'static [u8],
    ) -> String {
        let resp = upload(node, caller, context, Body::from(bytes)).await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "the upload itself must succeed"
        );
        let body = to_bytes(resp.into_body(), usize::MAX).await.expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        json["data"]["blobId"]
            .as_str()
            .or_else(|| json["data"]["blob_id"].as_str())
            .unwrap_or_else(|| panic!("no blob id in {json}"))
            .to_owned()
    }

    async fn download(
        node: &Node,
        caller: Caller,
        blob_id: &str,
        context: Option<ContextId>,
    ) -> Response {
        download_handler(
            Path(blob_id.to_owned()),
            Query(context_query(context)),
            Extension(Arc::clone(&node.state)),
            caller.owner(),
            caller.account(),
            None,
            HeaderMap::new(),
        )
        .await
        .into_response()
    }

    async fn head(
        node: &Node,
        caller: Caller,
        blob_id: &str,
        context: Option<ContextId>,
    ) -> Response {
        info_handler(
            Path(blob_id.to_owned()),
            Query(context_query(context)),
            Extension(Arc::clone(&node.state)),
            caller.owner(),
            caller.account(),
            None,
            HeaderMap::new(),
        )
        .await
        .into_response()
    }

    async fn body_of(resp: Response) -> Vec<u8> {
        to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body")
            .to_vec()
    }

    /// The criterion: a member uploads into its own context and reads the blob
    /// back through it, by GET and by HEAD.
    #[tokio::test]
    async fn a_member_uploads_and_downloads_through_its_own_context() {
        let node = node().await;
        let me = Caller::Account(ME);
        let blob_id = uploaded(&node, me, Some(my_context()), b"my attachment").await;

        let resp = download(&node, me, &blob_id, Some(my_context())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_of(resp).await, b"my attachment");

        let resp = head(&node, me, &blob_id, Some(my_context())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["X-Blob-Source"], "local");
    }

    /// A member of one context must not read another's blob: not by naming the
    /// other context (not a member), and not by naming its own (the blob is not
    /// that context's). The second answer is the same 404 as a missing blob, so
    /// it does not reveal that this node holds the bytes.
    #[tokio::test]
    async fn a_member_cannot_read_another_contexts_blob() {
        let node = node().await;
        let theirs = uploaded(
            &node,
            Caller::Account(THEM),
            Some(their_context()),
            b"their secret",
        )
        .await;
        let me = Caller::Account(ME);

        let resp = download(&node, me, &theirs, Some(their_context())).await;
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "not a member of that context"
        );

        let resp = download(&node, me, &theirs, Some(my_context())).await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "held here, but not for my context"
        );

        let resp = head(&node, me, &theirs, Some(my_context())).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "HEAD is no side door");
        assert!(!resp.headers().contains_key("X-Blob-Hash"));

        // And the tenant it belongs to still reads it.
        let resp = download(&node, Caller::Account(THEM), &theirs, Some(their_context())).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Dedup does not open a door: uploading the same bytes into my context
    /// associates them with my context — bytes I evidently already had — and
    /// leaves their context's association as it was.
    #[tokio::test]
    async fn uploading_identical_bytes_associates_only_the_uploaders_context() {
        let node = node().await;
        let theirs = uploaded(
            &node,
            Caller::Account(THEM),
            Some(their_context()),
            b"same bytes",
        )
        .await;
        let mine = uploaded(
            &node,
            Caller::Account(ME),
            Some(my_context()),
            b"same bytes",
        )
        .await;
        assert_eq!(theirs, mine, "content-addressed, so one blob");

        let resp = download(&node, Caller::Account(ME), &mine, Some(my_context())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = download(&node, Caller::Account(ME), &mine, Some(their_context())).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    /// A non-member cannot upload into a context, and nothing is stored.
    #[tokio::test]
    async fn a_non_member_is_refused_an_upload_into_the_context() {
        let node = node().await;
        let resp = upload(
            &node,
            Caller::Account(ME),
            Some(their_context()),
            Body::from("x"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(node
            .state
            .node_client
            .list_blobs()
            .expect("list")
            .is_empty());
    }

    /// An account must say which context a blob is for, on every route: with no
    /// context there is nothing to check its membership against.
    #[tokio::test]
    async fn an_account_must_name_a_context() {
        let node = node().await;
        let me = Caller::Account(ME);

        let resp = upload(&node, me, None, Body::from("x")).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let blob_id = uploaded(&node, me, Some(my_context()), b"mine").await;
        assert_eq!(
            download(&node, me, &blob_id, None).await.status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            head(&node, me, &blob_id, None).await.status(),
            StatusCode::BAD_REQUEST
        );
    }

    /// A context that no longer admits the caller stops serving it at once:
    /// membership is read per request, not from the session.
    #[tokio::test]
    async fn a_removed_member_stops_being_served() {
        let node = node().await;
        let me = Caller::Account(ME);
        let blob_id = uploaded(&node, me, Some(my_context()), b"mine").await;

        MembershipRepository::new(&node.state.store)
            .remove_member(&ContextGroupId::from([0xa1; 32]), &AccountId::from(ME))
            .expect("remove member");

        let resp = download(&node, me, &blob_id, Some(my_context())).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    /// An account's upload is held to the account ceiling, not the node's.
    #[tokio::test]
    async fn an_account_upload_over_its_ceiling_answers_413() {
        let node = node().await;
        let chunk = axum::body::Bytes::from(vec![0_u8; 1024 * 1024]);
        let chunks = usize::try_from(MAX_ACCOUNT_BLOB_UPLOAD_BYTES / (1024 * 1024)).unwrap() + 1;
        let stream = futures_util::stream::iter(
            std::iter::repeat_n(chunk, chunks).map(Ok::<_, std::io::Error>),
        );
        let resp = upload(
            &node,
            Caller::Account(ME),
            Some(my_context()),
            Body::from_stream(stream),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    /// Single-tenant operation is untouched: the node owner, and a node with no
    /// auth guard, upload and read with no context at all and read any blob.
    #[tokio::test]
    async fn a_node_wide_caller_is_unaffected() {
        let node = node().await;
        let theirs = uploaded(
            &node,
            Caller::Account(THEM),
            Some(their_context()),
            b"tenant bytes",
        )
        .await;

        for caller in [Caller::Owner, Caller::Unguarded] {
            let own = uploaded(&node, caller, None, b"operator bytes").await;
            for blob_id in [&own, &theirs] {
                let resp = download(&node, caller, blob_id, None).await;
                assert_eq!(resp.status(), StatusCode::OK);
                assert_eq!(
                    head(&node, caller, blob_id, None).await.status(),
                    StatusCode::OK
                );
            }
        }

        // An owner session that is also account-anchored keeps the owner's view.
        let resp = download_handler(
            Path(theirs.clone()),
            Query(context_query(None)),
            Extension(Arc::clone(&node.state)),
            Some(Extension(AuthenticatedNodeOwner)),
            Some(Extension(AuthenticatedAccount(AccountId::from(ME)))),
            None,
            HeaderMap::new(),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// An operator's upload for a context is readable by that context's
    /// delegated members, and by no one else's.
    #[tokio::test]
    async fn an_owner_upload_for_a_context_is_readable_by_its_members() {
        let node = node().await;
        let blob_id = uploaded(
            &node,
            Caller::Owner,
            Some(my_context()),
            b"from the operator",
        )
        .await;

        let resp = download(&node, Caller::Account(ME), &blob_id, Some(my_context())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = download(
            &node,
            Caller::Account(THEM),
            &blob_id,
            Some(their_context()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}

#[cfg(test)]
mod passive_cross_site_tests {
    use std::sync::Arc;

    use axum::extract::{Path, Query};
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::response::IntoResponse;
    use axum::Extension;
    use calimero_context_client::client::ContextClient;
    use calimero_node_primitives::test_fixtures;
    use calimero_primitives::blobs::BlobId;
    use calimero_primitives::context::ContextId;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;

    use super::{download_handler, info_handler};
    use crate::origin_guard::OriginGuard;
    use crate::{AdminState, NodeReadiness};

    const BLOB: &[u8] = b"held by a context peer";
    const NAVIGATION: [(&str, &str); 2] = [
        ("sec-fetch-site", "cross-site"),
        ("sec-fetch-mode", "navigate"),
    ];
    const IMAGE: [(&str, &str); 2] = [
        ("sec-fetch-site", "cross-site"),
        ("sec-fetch-mode", "no-cors"),
    ];

    /// A proxy-mode node that holds nothing, in a context whose one peer holds `BLOB`.
    struct Node {
        state: Arc<AdminState>,
        blob_id: BlobId,
        _dirs: [tempfile::TempDir; 4],
    }

    async fn node() -> Node {
        let (holder, _store, holder_data, holder_blobs) = test_fixtures::node_client().await;
        let (blob_id, _size) = holder.add_blob(BLOB, None, None).await.unwrap();

        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (client, data, blobs) = test_fixtures::node_client_over(
            store.clone(),
            test_fixtures::network_of_one_peer(Some(BLOB.to_vec())),
        )
        .await;
        let state = Arc::new(AdminState::new(
            store.clone(),
            ContextClient::new(store, client.clone(), LazyRecipient::new()),
            client,
            Arc::new(NodeReadiness::new()),
            [0; 32],
            #[cfg(feature = "mock-attestation")]
            false,
        ));

        Node {
            state,
            blob_id,
            _dirs: [holder_data, holder_blobs, data, blobs],
        }
    }

    /// The answer to a request naming the node by its own address, and whether the
    /// node holds the blob afterwards. The origin guard admits the navigation.
    async fn request(
        node: &Node,
        head: bool,
        headers: &[(&'static str, &'static str)],
    ) -> (StatusCode, bool) {
        let mut sent = HeaderMap::new();
        let _ = sent.insert("host", HeaderValue::from_static("127.0.0.1:2528"));
        for (name, value) in headers {
            let _ = sent.insert(*name, HeaderValue::from_static(value));
        }
        if headers == NAVIGATION {
            assert!(OriginGuard::new(None).admits(&sent, None));
        }

        let path = Path(node.blob_id.to_string());
        let query = Query(
            [(
                "context_id".to_owned(),
                ContextId::from([0xA1; 32]).to_string(),
            )]
            .into_iter()
            .collect(),
        );
        let state = Extension(Arc::clone(&node.state));
        let response = if head {
            info_handler(path, query, state, None, None, None, sent)
                .await
                .into_response()
        } else {
            download_handler(path, query, state, None, None, None, sent)
                .await
                .into_response()
        };
        (
            response.status(),
            node.state.node_client.has_blob(&node.blob_id).unwrap(),
        )
    }

    /// A page on another site can make the operator's browser navigate to a blob
    /// URL or load it as an image; neither may make the node ask its peers.
    #[tokio::test]
    async fn a_passive_request_from_another_site_fetches_nothing_from_peers() {
        let node = node().await;

        for headers in [NAVIGATION, IMAGE] {
            assert_eq!(
                request(&node, false, &headers).await,
                (StatusCode::NOT_FOUND, false),
                "GET {headers:?}"
            );
            assert_eq!(
                request(&node, true, &headers).await,
                (StatusCode::NOT_FOUND, false),
                "HEAD {headers:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_script_and_a_client_that_is_no_browser_still_fetch_from_peers() {
        let app_fetch = [
            ("origin", "http://localhost:5173"),
            ("sec-fetch-site", "cross-site"),
            ("sec-fetch-mode", "cors"),
        ];
        let own_page_link = [
            ("sec-fetch-site", "same-origin"),
            ("sec-fetch-mode", "navigate"),
        ];
        let address_bar = [("sec-fetch-site", "none"), ("sec-fetch-mode", "navigate")];

        for headers in [&app_fetch[..], &own_page_link, &address_bar, &[]] {
            let node = node().await;
            assert_eq!(
                request(&node, true, headers).await,
                (StatusCode::OK, false),
                "HEAD {headers:?}"
            );
            assert_eq!(
                request(&node, false, headers).await,
                (StatusCode::OK, true),
                "GET {headers:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_blob_already_held_is_served_to_a_passive_request_from_another_site() {
        let node = node().await;
        let _fetched = request(&node, false, &[]).await;

        assert_eq!(
            request(&node, false, &NAVIGATION).await,
            (StatusCode::OK, true)
        );
    }
}
