use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};

use crate::admin::service::{AdminState, ApiResponse};

pub async fn install_dev_application_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<calimero_server_primitives::admin::InstallDevApplicationRequest>,
) -> impl IntoResponse {
    match state
        .ctx_manager
        .install_dev_application(req.application_id, &req.version, req.path)
        .await
    {
        Ok(()) => ApiResponse { payload: () }.into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}
