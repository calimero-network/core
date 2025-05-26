use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{
    UninstallApplicationRequest, UninstallApplicationResponse,
};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<UninstallApplicationRequest>,
) -> impl IntoResponse {
    match state.node_client.uninstall_application(req.application_id) {
        Ok(()) => ApiResponse {
            payload: UninstallApplicationResponse::new(req.application_id),
        }
        .into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}
