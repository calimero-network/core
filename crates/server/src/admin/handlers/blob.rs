use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use calimero_primitives::blobs::BlobId;
use serde::{Deserialize, Serialize};

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

#[derive(Debug, Deserialize)]
pub struct BlobUploadQuery {
    /// Expected hash of the blob for verification (optional)
    hash: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BlobUploadResponse {
    /// The unique blob ID
    blob_id: String,
    /// Size of the uploaded blob in bytes
    size: u64,
}

#[derive(Debug, Serialize)]
pub struct BlobMetadata {
    /// The blob ID
    blob_id: String,
    /// Size of the blob in bytes
    size: u64,
    /// Whether the blob exists
    exists: bool,
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

/// Upload a blob via streaming
///
/// This endpoint accepts raw binary data as the request body and stores it as a blob.
/// The blob can then be referenced by its returned ID in application calls.
pub async fn upload_handler(
    Query(query): Query<BlobUploadQuery>,
    Extension(state): Extension<Arc<AdminState>>,
    body: Body,
) -> impl IntoResponse {
    use axum::body::to_bytes;

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

    // Convert body to bytes then to a slice for add_blob
    let bytes = match to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(err) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: format!("Failed to read request body: {}", err),
            }
            .into_response();
        }
    };

    // Store the blob using the node client
    match state
        .node_client
        .add_blob(&bytes[..], None, expected_hash.as_ref())
        .await
    {
        Ok((blob_id, size)) => ApiResponse {
            payload: BlobUploadResponse {
                blob_id: blob_id.to_string(),
                size,
            },
        }
        .into_response(),
        Err(err) => {
            tracing::error!("Failed to upload blob: {:?}", err);
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

/// Get blob metadata
///
/// Returns information about the blob including its size and existence.
pub async fn metadata_handler(
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
            payload: BlobMetadata {
                blob_id: blob_id.to_string(),
                size: blob_data.len() as u64,
                exists: true,
            },
        }
        .into_response(),
        Ok(None) => ApiResponse {
            payload: BlobMetadata {
                blob_id: blob_id.to_string(),
                size: 0,
                exists: false,
            },
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

// Keep the old handler name for backward compatibility
pub use upload_handler as handler;
