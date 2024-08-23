use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::{
    GetApplicationResponse, InstallApplicationResponse, InstallDevApplicationRequest,
};

use crate::admin::service::{AdminState, ApiResponse};

pub async fn install_dev_application_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<InstallDevApplicationRequest>,
) -> impl IntoResponse {
    match state
        .ctx_manager
        .install_application_from_path(req.path, req.version, req.metadata)
        .await
    {
        Ok(application_id) => ApiResponse {
            payload: InstallApplicationResponse::new(application_id),
        }
        .into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

pub async fn get_application(
    Extension(state): Extension<Arc<AdminState>>,
    Path(application_id): Path<ApplicationId>,
) -> impl IntoResponse {
    match state.ctx_manager.get_application(&application_id) {
        Ok(application) => ApiResponse {
            payload: GetApplicationResponse::new(application),
        }
        .into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}
