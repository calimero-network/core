use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::UninstallApplicationResponse;
use tracing::{error, info};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Path(application_id): Path<ApplicationId>,
) -> impl IntoResponse {
    info!(application_id=%application_id, "Uninstalling application");

    match state.node_client.uninstall_application(&application_id) {
        Ok(()) => {
            info!(application_id=%application_id, "Application uninstalled successfully");
            ApiResponse {
                payload: UninstallApplicationResponse::new(application_id),
            }
            .into_response()
        }
        Err(err) => {
            error!(application_id=%application_id, error=?err, "Failed to uninstall application");
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
        }
    }
}
