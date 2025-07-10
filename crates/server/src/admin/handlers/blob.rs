use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use calimero_primitives::blobs::BlobId;
use futures_util::{AsyncRead, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tokio_util::io::StreamReader;

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

#[derive(Debug, Deserialize)]
pub struct BlobUploadQuery {
    /// Expected hash of the blob for verification (optional)
    hash: Option<String>,
}

#[derive(Debug, Serialize, Copy, Clone)]
pub struct BlobInfo {
    /// The unique blob ID
    blob_id: BlobId,
    /// Size of the blob in bytes
    size: u64,
}

/// Detect MIME type from file content
fn detect_mime_type(data: &[u8]) -> &'static str {
    // Check for common file signatures
    if data.len() >= 4 {
        match &data[0..4] {
            [0x89, 0x50, 0x4E, 0x47] => return "image/png",
            [0xFF, 0xD8, 0xFF, _] => return "image/jpeg",
            [0x47, 0x49, 0x46, 0x38] => return "image/gif",
            [0x52, 0x49, 0x46, 0x46] if data.len() >= 12 && &data[8..12] == b"WEBP" => {
                return "image/webp"
            }
            [0x25, 0x50, 0x44, 0x46] => return "application/pdf",
            _ => {}
        }
    }

    if data.len() >= 3 && &data[0..3] == [0xFF, 0xD8, 0xFF] {
        return "image/jpeg";
    }

    if data.len() >= 8 {
        // Check for ZIP files
        if &data[0..4] == [0x50, 0x4B, 0x03, 0x04] || &data[0..4] == [0x50, 0x4B, 0x05, 0x06] {
            return "application/zip";
        }
    }

    // Check if it's valid UTF-8 text
    if let Ok(text) = std::str::from_utf8(data) {
        // Check for common text file patterns
        if text.trim_start().starts_with("<!DOCTYPE html") || text.trim_start().starts_with("<html")
        {
            return "text/html";
        }
        if text.trim_start().starts_with("{") || text.trim_start().starts_with("[") {
            // Basic JSON detection
            if serde_json::from_str::<serde_json::Value>(text).is_ok() {
                return "application/json";
            }
        }
        if text.contains("<?xml") {
            return "application/xml";
        }
        // Default to plain text for valid UTF-8
        return "text/plain";
    }

    // Default to binary
    "application/octet-stream"
}

/// Convert axum Body to futures AsyncRead using tokio_util::io::StreamReader
/// This allows streaming large files without loading them entirely into memory
fn body_to_async_read(body: Body) -> impl AsyncRead {
    // Convert Body to a stream of Result<Bytes, Error>
    let byte_stream = body
        .into_data_stream()
        .map(|result| result.map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err)));

    // Use StreamReader to convert Stream<Item = Result<Bytes, Error>> to tokio AsyncRead
    // Then convert to futures AsyncRead using compat()
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
    // Parse expected hash if provided
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

    // Create streaming reader from the body
    let reader = body_to_async_read(body);

    // Store the blob using the node client with streaming
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

/// Download a blob by its ID
///
/// Returns the raw binary data of the blob with appropriate content headers.
pub async fn download_handler(
    Path(blob_id): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    // Parse blob ID
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

    // Get blob data
    match state.node_client.get_blob_bytes(&blob_id).await {
        Ok(Some(blob_data)) => {
            // Detect MIME type from content
            let content_type = detect_mime_type(&blob_data);

            // Generate a more descriptive filename based on content type
            let filename = match content_type {
                "image/png" => format!("blob-{}.png", &blob_id.to_string()[..8]),
                "image/jpeg" => format!("blob-{}.jpg", &blob_id.to_string()[..8]),
                "image/gif" => format!("blob-{}.gif", &blob_id.to_string()[..8]),
                "image/webp" => format!("blob-{}.webp", &blob_id.to_string()[..8]),
                "application/pdf" => format!("blob-{}.pdf", &blob_id.to_string()[..8]),
                "application/zip" => format!("blob-{}.zip", &blob_id.to_string()[..8]),
                "text/html" => format!("blob-{}.html", &blob_id.to_string()[..8]),
                "application/json" => format!("blob-{}.json", &blob_id.to_string()[..8]),
                "application/xml" => format!("blob-{}.xml", &blob_id.to_string()[..8]),
                "text/plain" => format!("blob-{}.txt", &blob_id.to_string()[..8]),
                _ => format!("blob-{}", &blob_id.to_string()[..8]),
            };

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::CONTENT_LENGTH, blob_data.len())
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", filename),
                )
                .body(Body::from(blob_data.to_vec()))
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

/// Get blob info
///
/// Returns information about the blob including its size.
pub async fn info_handler(
    Path(blob_id): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    // Parse blob ID
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

    // Check if blob exists and get its size
    match state.node_client.get_blob_bytes(&blob_id).await {
        Ok(Some(blob_data)) => ApiResponse {
            payload: BlobInfo {
                blob_id,
                size: blob_data.len() as u64,
            },
        }
        .into_response(),
        Ok(None) => ApiError {
            status_code: StatusCode::NOT_FOUND,
            message: "Blob not found".to_owned(),
        }
        .into_response(),
        Err(err) => {
            tracing::error!("Failed to check blob {}: {:?}", blob_id, err);
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to check blob: {}", err),
            }
            .into_response()
        }
    }
}

// Export the handler with the old name for backward compatibility
pub use upload_handler as handler;
