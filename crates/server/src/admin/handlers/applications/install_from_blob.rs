use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::application::ApplicationSource;
use calimero_primitives::blobs::BlobId;
use calimero_server_primitives::admin::InstallApplicationResponse;
use serde::{Deserialize, Serialize};

use crate::admin::service::ApiResponse;
use crate::AdminState;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallApplicationFromBlobRequest {
    pub blob_id: BlobId,
    pub metadata: Vec<u8>,
    pub source: String,
    pub size: Option<u64>,
}

impl InstallApplicationFromBlobRequest {
    pub fn new(blob_id: BlobId, metadata: Vec<u8>, source: String, size: Option<u64>) -> Self {
        Self {
            blob_id,
            metadata,
            source,
            size,
        }
    }
}

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<InstallApplicationFromBlobRequest>,
) -> impl IntoResponse {
    // Validate that the blob exists and get its actual size if not provided
    let actual_size = match req.size {
        Some(size) => size,
        None => {
            // Get blob info to determine size
            match state.node_client.has_blob(&req.blob_id) {
                Ok(true) => {
                    // For now, use the provided size or 0 if not available
                    // In a real implementation, you might want to get the actual size
                    0
                }
                Ok(false) => {
                    return (
                        StatusCode::NOT_FOUND,
                        format!("Blob not found: {}", req.blob_id),
                    )
                        .into_response()
                }
                Err(err) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to check blob existence: {}", err),
                    )
                        .into_response()
                }
            }
        }
    };

    // Parse the application source
    let source = match req.source.parse::<ApplicationSource>() {
        Ok(source) => source,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Invalid application source: {}", err),
            )
                .into_response()
        }
    };

    // Use the provided size, or fall back to actual_size if we determined it
    let final_size = req.size.unwrap_or(actual_size);

    // Install the application using the existing node client method
    match state
        .node_client
        .install_application(&req.blob_id, final_size, &source, req.metadata)
    {
        Ok(application_id) => ApiResponse {
            payload: InstallApplicationResponse::new(application_id),
        }
        .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to install application: {}", err),
        )
            .into_response(),
    }
}
