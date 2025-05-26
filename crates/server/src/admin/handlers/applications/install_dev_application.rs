use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{InstallApplicationResponse, InstallDevApplicationRequest};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<InstallDevApplicationRequest>,
) -> impl IntoResponse {
    match state
        .node_client
        .install_application_from_path(req.path, req.metadata)
        .await
    {
        Ok(application_id) => ApiResponse {
            payload: InstallApplicationResponse::new(application_id),
        }
        .into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}
