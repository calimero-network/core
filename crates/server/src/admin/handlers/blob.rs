use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query};
use axum::http::response::Builder;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use calimero_primitives::blobs::{BlobId, BlobInfo};
use futures_util::{AsyncRead, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tokio_util::io::StreamReader;

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

#[derive(Debug, Deserialize)]
pub struct BlobUploadQuery {
    /// Expected hash of the blob for verification
    hash: Option<String>,
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
    match state.node_client.list_blobs() {
        Ok(blobs) => ApiResponse {
            payload: BlobListResponse {
                data: BlobListResponseData { blobs },
            },
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}

/// Helper function to build response headers from blob metadata
fn build_blob_response_headers(
    blob_metadata: &calimero_primitives::blobs::BlobMetadata,
    blob_id: BlobId,
) -> Builder {
    let etag = format!("\"{}\"", hex::encode(blob_metadata.hash));

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Length", blob_metadata.size.to_string())
        .header("Content-Type", &blob_metadata.mime_type)
        .header("ETag", &etag)
        .header("Cache-Control", "public, max-age=3600") // 1 hour cache
        .header("X-Blob-ID", blob_id.to_string())
        .header("X-Blob-Hash", hex::encode(blob_metadata.hash))
        .header("X-Blob-MIME-Type", &blob_metadata.mime_type)
        .header(
            "Access-Control-Expose-Headers",
            "X-Blob-ID, X-Blob-Hash, X-Blob-MIME-Type, ETag",
        )
}

/// Download a blob by its ID
///
/// Returns the raw binary data of the blob with complete metadata headers.
/// Headers are identical to HEAD request for the same blob.
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

    // Get blob metadata first to include in headers
    let blob_metadata = match state.node_client.get_blob_info(blob_id).await {
        Ok(Some(metadata)) => metadata,
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
                message: format!("Failed to retrieve blob metadata: {}", err),
            }
            .into_response();
        }
    };

    // Get blob stream
    match state.node_client.get_blob(&blob_id) {
        Ok(Some(blob)) => {
            tracing::debug!(
                "Serving blob {} via streaming with metadata headers",
                blob_id
            );

            let stream = blob.map(|result| {
                result.map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))
            });

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

    match state.node_client.delete_blob(blob_id).await {
        Ok(deleted) => ApiResponse {
            payload: BlobDeleteResponse { blob_id, deleted },
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
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

    match state.node_client.get_blob_info(blob_id).await {
        Ok(Some(blob_metadata)) => build_blob_response_headers(&blob_metadata, blob_id)
            .body(Body::empty())
            .unwrap_or_else(|_| {
                ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "Failed to build response".to_owned(),
                }
                .into_response()
            }),
        Ok(None) => ApiError {
            status_code: StatusCode::NOT_FOUND,
            message: "Blob not found".to_owned(),
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}
