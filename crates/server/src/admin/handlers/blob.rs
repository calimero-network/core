use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query};
use axum::http::response::Builder;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use calimero_node_primitives::client::BlobPresence;
use calimero_primitives::blobs::{BlobId, BlobInfo, BlobMetadata};
use calimero_primitives::content_hash::ContentHash;
use calimero_primitives::hash::Hash;
use futures_util::{AsyncRead, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tokio_util::io::StreamReader;
use tracing::{debug, error, info};

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

#[derive(Debug, Deserialize)]
pub struct BlobUploadQuery {
    /// Expected hash of the blob for verification
    hash: Option<String>,
    /// Context ID to announce the blob to for network discovery
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

/// Convert axum Body to futures AsyncRead using tokio_util::io::StreamReader.
/// This allows streaming large files without loading them entirely into memory.
///
/// The stream errors out once cumulative bytes exceed [`MAX_BLOB_UPLOAD_BYTES`],
/// so an oversized (or unbounded/chunked) upload is aborted mid-stream rather
/// than being written to disk in full.
fn body_to_async_read(body: Body) -> impl AsyncRead {
    let mut total: u64 = 0;
    let byte_stream = body.into_data_stream().map(move |result| {
        let chunk = result.map_err(std::io::Error::other)?;
        total = total.saturating_add(chunk.len() as u64);
        if total > MAX_BLOB_UPLOAD_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "blob upload exceeds maximum allowed size",
            ));
        }
        Ok(chunk)
    });

    StreamReader::new(byte_stream).compat()
}

// Clients send `?hash=` as 64 hex, like every other `Hash` on the wire; only the
// digest bytes carry over into the `ContentHash` that `add_blob` compares against.
fn parse_expected_content_hash(hash_str: &str) -> Option<ContentHash> {
    let hash: Hash = hash_str.parse().ok()?;
    Some(ContentHash::from(*hash.as_bytes()))
}

/// Upload a blob via raw binary data (streaming version)
///
/// This endpoint accepts raw binary data in the request body and streams it
/// directly to blob storage without loading it all into memory first.
/// Perfect for large file uploads with minimal memory usage.
///
/// Query parameters:
/// - `hash`: Expected hash of the blob for verification (optional)
/// - `context_id`: Context ID to announce the blob to for network discovery (optional)
pub async fn upload_handler(
    Query(query): Query<BlobUploadQuery>,
    Extension(state): Extension<Arc<AdminState>>,
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

    info!("Uploading blob");
    debug!(has_expected_hash=%expected_hash.is_some(), has_context=%context_id.is_some(), "Blob upload request");

    let reader = body_to_async_read(body);

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

            // Announce blob to network if context_id is provided.
            //
            // Announcing writes a discovery record advertising that this node
            // holds the blob for the given context. Only announce for a context
            // this node actually participates in: without this check a caller
            // could inject blob-availability records into arbitrary contexts'
            // discovery. Membership is proven by the node owning an identity in
            // the context (the same signal the signed serving path relies on).
            if let Some(ctx_id) = context_id {
                match state.node_client.find_owned_identity(&ctx_id) {
                    Ok(Some(_)) => match state
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
                    },
                    Ok(None) => {
                        error!(blob_id=%blob_id, context_id=%ctx_id, "Skipping announce: node is not a member of the requested context");
                    }
                    Err(err) => {
                        error!(blob_id=%blob_id, context_id=%ctx_id, error=?err, "Skipping announce: failed to verify context membership");
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
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
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
pub async fn download_handler(
    Path(blob_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    Extension(state): Extension<Arc<AdminState>>,
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

    let blob_result = state
        .node_client
        .get_blob(&blob_id, context_id.as_ref())
        .await;

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
pub async fn info_handler(
    Path(blob_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
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

    let presence = state
        .node_client
        .get_blob_presence(blob_id, context_id.as_ref())
        .await;

    let headers = match presence {
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
