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

/// Get blob info
///
/// Returns information about the blob including its size.
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

pub use upload_handler as handler;
