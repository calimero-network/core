use std::sync::Arc;

use axum::body::Body;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{
    InstallApplicationResponse, InstallApplicationStreamJsonRequest,
    InstallApplicationStreamRequest,
};
use tokio_util::compat::TokioAsyncReadCompatExt;

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Query(req): Query<InstallApplicationStreamRequest>,
    body: Body,
) -> impl IntoResponse {
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Failed to read request body: {}", err),
            )
                .into_response()
        }
    };

    let metadata = match req.decode_metadata() {
        Ok(metadata) => metadata,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Invalid metadata: {}", err),
            )
                .into_response()
        }
    };

    let cursor = std::io::Cursor::new(bytes.to_vec()).compat();

    match state
        .node_client
        .install_application_from_stream(
            cursor,
            req.expected_size,
            req.expected_hash.as_ref(),
            metadata,
        )
        .await
    {
        Ok(application_id) => ApiResponse {
            payload: InstallApplicationResponse::new(application_id),
        }
        .into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

pub async fn json_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<InstallApplicationStreamJsonRequest>,
) -> impl IntoResponse {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    let app_data = match STANDARD.decode(&req.application_data) {
        Ok(data) => data,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Invalid base64 data: {}", err),
            )
                .into_response()
        }
    };

    let cursor = std::io::Cursor::new(app_data).compat();

    match state
        .node_client
        .install_application_from_stream(
            cursor,
            req.expected_size,
            req.expected_hash.as_ref(),
            req.metadata,
        )
        .await
    {
        Ok(application_id) => ApiResponse {
            payload: InstallApplicationResponse::new(application_id),
        }
        .into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

