use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use calimero_primitives::blobs::BlobId;
use calimero_store::key::BlobMeta as BlobMetaKey;
use calimero_store::layer::LayerExt;
use futures_util::{AsyncRead, StreamExt};
use hex;
use serde::{Deserialize, Serialize};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tokio_util::io::StreamReader;

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

#[derive(Debug, Deserialize)]
pub struct BlobUploadQuery {
    /// Expected hash of the blob for verification
    hash: Option<String>,
}

#[derive(Debug, Serialize, Clone, Copy)]
pub struct BlobInfo {
    /// The unique blob ID
    blob_id: BlobId,
    /// Size of the blob in bytes
    size: u64,
}

/// Convert axum Body to futures AsyncRead using tokio_util::io::StreamReader
/// This allows streaming large files without loading them entirely into memory
fn body_to_async_read(body: Body) -> impl AsyncRead {
    let byte_stream = body
        .into_data_stream()
        .map(|result| result.map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err)));

    StreamReader::new(byte_stream).compat()
}

/// Upload a blob via raw binary data (streaming version)
///
/// This endpoint accepts raw binary data in the request body and streams it
/// directly to blob storage without loading it all into memory first.
/// Perfect for large file uploads with minimal memory usage.
pub async fn upload_handler(
    Query(query): Query<BlobUploadQuery>,
    Extension(state): Extension<Arc<AdminState>>,
    body: Body,
) -> impl IntoResponse {
    let expected_hash = if let Some(hash_str) = query.hash {
        match hash_str.parse() {
            Ok(hash) => Some(hash),
            Err(_) => {
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

    tracing::info!("Starting streaming raw blob upload");

    let reader = body_to_async_read(body);

    match state
        .node_client
        .add_blob(reader, None, expected_hash.as_ref())
        .await
    {
        Ok((blob_id, size)) => {
            tracing::info!(
                "Successfully uploaded streaming blob {} with size {} bytes ({:.1} MB)",
                blob_id,
                size,
                size as f64 / (1024.0 * 1024.0)
            );
            ApiResponse {
                payload: BlobInfo { blob_id, size },
            }
            .into_response()
        }
        Err(err) => {
            tracing::error!("Failed to upload streaming blob: {:?}", err);
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to store blob: {}", err),
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
/// This excludes individual chunk blobs to provide a cleaner user experience.
pub async fn list_handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    let handle = state.store.clone().handle();

    let iter_result = handle.iter::<BlobMetaKey>();
    let mut iter = match iter_result {
        Ok(iter) => iter,
        Err(err) => {
            tracing::error!("Failed to create blob iterator: {:?}", err);
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to iterate blob entries".to_owned(),
            }
            .into_response();
        }
    };

    let mut chunk_blob_ids = std::collections::HashSet::new();

    tracing::debug!("Starting first pass: collecting chunk blob IDs");
    for result in iter.entries() {
        match result {
            (Ok(_blob_key), Ok(blob_meta)) => {
                // Only collect chunk IDs, not full blob info
                for link in blob_meta.links.iter() {
                    let _ =chunk_blob_ids.insert(link.blob_id());
                }
            }
            (Err(err), _) | (_, Err(err)) => {
                tracing::error!("Failed to read blob entry during chunk collection: {:?}", err);
                return ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "Failed to read blob entries".to_owned(),
                }
                .into_response();
            }
        }
    }

    let iter_result2 = handle.iter::<BlobMetaKey>();
    let mut iter2 = match iter_result2 {
        Ok(iter) => iter,
        Err(err) => {
            tracing::error!("Failed to create second blob iterator: {:?}", err);
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to iterate blob entries".to_owned(),
            }
            .into_response();
        }
    };

    let mut root_blobs = Vec::new();

    tracing::debug!("Starting second pass: collecting root blobs (filtering {} chunks)", chunk_blob_ids.len());
    for result in iter2.entries() {
        match result {
            (Ok(blob_key), Ok(blob_meta)) => {
                let blob_id = blob_key.blob_id();
                
                // Only include if it's not a chunk blob
                if !chunk_blob_ids.contains(&blob_id) {
                    root_blobs.push(BlobInfo { blob_id, size: blob_meta.size });
                }
            }
            (Err(err), _) | (_, Err(err)) => {
                tracing::error!("Failed to read blob entry during root collection: {:?}", err);
                return ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "Failed to read blob entries".to_owned(),
                }
                .into_response();
            }
        }
    }

    tracing::debug!(
        "Listing complete: found {} chunks, returning {} root/standalone blobs",
        chunk_blob_ids.len(),
        root_blobs.len()
    );

    ApiResponse {
        payload: BlobListResponse {
            data: BlobListResponseData { blobs: root_blobs },
        },
    }
    .into_response()
}

#[derive(Debug, Serialize)]
pub struct BlobListResponse {
    /// Wrapped response data
    data: BlobListResponseData,
}

#[derive(Debug, Serialize)]
pub struct BlobListResponseData {
    /// List of all blobs
    blobs: Vec<BlobInfo>,
}

/// Download a blob by its ID
///
/// Returns the raw binary data of the blob as application/octet-stream.
pub async fn download_handler(
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

    match state.node_client.get_blob(&blob_id) {
        Ok(Some(blob)) => {
            tracing::debug!("Serving blob {} via streaming", blob_id);

            let stream = blob.map(|result| {
                result.map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))
            });

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from_stream(stream))
                .unwrap()
        }
        Ok(None) => ApiError {
            status_code: StatusCode::NOT_FOUND,
            message: "Blob not found".to_owned(),
        }
        .into_response(),
        Err(err) => {
            tracing::error!("Failed to retrieve blob {}: {:?}", blob_id, err);
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to retrieve blob: {}", err),
            }
            .into_response()
        }
    }
}

/// Delete a blob metadata by its ID
///
/// Removes blob metadata from database only (files remain on disk for safety).
/// This includes metadata for all associated chunk files for large blobs.
/// Actual blob files are preserved to prevent breaking applications.
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

    let mut handle = state.store.clone().handle();
    let blob_key = BlobMetaKey::new(blob_id);

    let blob_meta = match handle.get(&blob_key) {
        Ok(Some(meta)) => meta,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Blob not found".to_owned(),
            }
            .into_response();
        }
        Err(err) => {
            tracing::error!("Failed to get blob metadata {}: {:?}", blob_id, err);
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to access blob metadata: {}", err),
            }
            .into_response();
        }
    };

    tracing::info!(
        "Starting metadata deletion for blob {} with {} linked chunks",
        blob_id,
        blob_meta.links.len()
    );

    let mut blobs_to_delete = vec![blob_id];
    let mut deleted_count = 0;

    blobs_to_delete.extend(blob_meta.links.iter().map(|link| link.blob_id()));

    for current_blob_id in blobs_to_delete {
        let current_key = BlobMetaKey::new(current_blob_id);

        match handle.delete(&current_key) {
            Ok(()) => {
                deleted_count += 1;
                tracing::debug!(
                    "Successfully deleted metadata for blob {} (file remains on disk)",
                    current_blob_id
                );
            }
            Err(err) => {
                tracing::warn!(
                    "Failed to delete metadata for blob {}: {}",
                    current_blob_id,
                    err
                );
            }
        }
    }

    if deleted_count > 0 {
        tracing::info!(
            "Successfully deleted metadata for {} blob(s) including chunks (files remain on disk)",
            deleted_count
        );
        ApiResponse {
            payload: BlobDeleteResponse {
                blob_id,
                deleted: true,
            },
        }
        .into_response()
    } else {
        ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Failed to delete any blob metadata".to_owned(),
        }
        .into_response()
    }
}

#[derive(Debug, Serialize, Copy, Clone)]
pub struct BlobDeleteResponse {
    blob_id: BlobId,
    deleted: bool,
}

/// Get blob metadata via HEAD request
///
/// Returns blob metadata in HTTP headers without the actual blob content.
/// This is efficient for checking blob existence and getting size info.
/// Also detects and returns MIME type based on file content.
pub async fn info_handler(
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

    let handle = state.store.clone().handle();
    let blob_key = BlobMetaKey::new(blob_id);

    match handle.get(&blob_key) {
        Ok(Some(blob_meta)) => {
            let content_type = detect_mime_type(&state, &blob_id)
                .await
                .unwrap_or_else(|| "application/octet-stream".to_string());
            let etag = format!("\"{}\"", hex::encode(blob_meta.hash));

            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Length", blob_meta.size.to_string())
                .header("Content-Type", &content_type)
                .header("ETag", &etag)
                .header("Cache-Control", "public, max-age=3600") // 1 hour cache
                .header("X-Blob-ID", blob_id.to_string())
                .header("X-Blob-Hash", hex::encode(blob_meta.hash))
                .header("X-Blob-MIME-Type", &content_type)
                .header(
                    "Access-Control-Expose-Headers",
                    "X-Blob-ID, X-Blob-Hash, X-Blob-MIME-Type, ETag",
                )
                .body(Body::empty())
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
            message: "Blob not found".to_owned(),
        }
        .into_response(),
        Err(err) => {
            tracing::error!("Failed to get blob metadata: {:?}", err);
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to retrieve blob metadata: {}", err),
            }
            .into_response()
        }
    }
}

/// Detect MIME type by reading the first few bytes of a blob
async fn detect_mime_type(state: &AdminState, blob_id: &BlobId) -> Option<String> {
    match state.node_client.get_blob(blob_id) {
        Ok(Some(mut blob_stream)) => {
            use futures_util::StreamExt;

            if let Some(Ok(first_chunk)) = blob_stream.next().await {
                let bytes = first_chunk.as_ref();
                let sample_size = std::cmp::min(bytes.len(), 20);
                return Some(detect_mime_from_bytes(&bytes[..sample_size]));
            }
        }
        Ok(None) => {
            tracing::warn!("Blob {} not found for MIME detection", blob_id);
        }
        Err(err) => {
            tracing::warn!(
                "Failed to read blob {} for MIME detection: {:?}",
                blob_id,
                err
            );
        }
    }

    None
}

/// Detect MIME type from file magic bytes
fn detect_mime_from_bytes(bytes: &[u8]) -> String {
    if bytes.len() < 4 {
        return "application/octet-stream".to_string();
    }

    let hex: String = bytes
        .iter()
        .take(20)
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    // File signatures (magic bytes) - most specific first
    if hex.starts_with("0061736d") {
        return "application/wasm".to_string();
    }
    if hex.starts_with("ffd8ff") {
        return "image/jpeg".to_string();
    }
    if hex.starts_with("89504e47") {
        return "image/png".to_string();
    }
    if hex.starts_with("47494638") {
        return "image/gif".to_string();
    }
    if hex.starts_with("25504446") {
        return "application/pdf".to_string();
    }
    if hex.starts_with("504b0304") || hex.starts_with("504b0506") || hex.starts_with("504b0708") {
        return "application/zip".to_string();
    }
    if hex.starts_with("1f8b08") {
        return "application/gzip".to_string();
    }
    if hex.starts_with("7f454c46") {
        return "application/x-executable".to_string();
    }
    if hex.starts_with("cafebabe") {
        return "application/java-vm".to_string();
    }
    if hex.starts_with("4d5a") {
        return "application/x-msdownload".to_string();
    }
    if hex.starts_with("377abcaf271c") {
        return "application/x-7z-compressed".to_string();
    }
    if hex.starts_with("425a68") {
        return "application/x-bzip2".to_string();
    }
    if hex.starts_with("526172211a0700") || hex.starts_with("526172211a070100") {
        return "application/vnd.rar".to_string();
    }

    if let Ok(text) = std::str::from_utf8(bytes) {
        if text.starts_with("<!DOCTYPE") || text.starts_with("<html") {
            return "text/html".to_string();
        }
        if text.starts_with('{') || text.starts_with('[') {
            return "application/json".to_string();
        }
        if text.contains("function") || text.contains("const ") || text.contains("var ") {
            return "text/javascript".to_string();
        }
        if text
            .chars()
            .all(|c| c.is_ascii() && (c.is_ascii_graphic() || c.is_ascii_whitespace()))
        {
            return "text/plain".to_string();
        }
    }

    "application/octet-stream".to_string()
}
