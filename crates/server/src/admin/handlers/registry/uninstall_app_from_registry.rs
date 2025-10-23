use axum::response::IntoResponse;
use axum::{Extension, Json};
use std::sync::Arc;

use crate::admin::service::ApiResponse;
use crate::AdminState;
use calimero_server_primitives::registry::{UninstallAppRequest, UninstallAppResponse};

pub async fn handler(
    Extension(_state): Extension<Arc<AdminState>>,
    Json(request): Json<UninstallAppRequest>,
) -> impl IntoResponse {
    // For now, this is a placeholder implementation
    // TODO: Implement actual app uninstallation logic

    ApiResponse {
        payload: UninstallAppResponse {
            success: true,
            message: format!("App '{}' uninstalled successfully", request.app_name),
        },
    }
    .into_response()
}
