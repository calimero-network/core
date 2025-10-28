use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{InstallApplicationResponse, InstallDevApplicationRequest};
use tracing::{error, info};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<InstallDevApplicationRequest>,
) -> impl IntoResponse {
    info!(path=%req.path, "Installing dev application");

    match state
        .node_client
        .install_application_from_path(
            req.path.clone(),
            req.metadata,
            req.package.as_deref().unwrap_or("unknown"),
            req.version.as_deref().unwrap_or("0.0.0"),
        )
        .await
    {
        Ok(application_id) => {
            info!(application_id=%application_id, "Dev application installed successfully");
            ApiResponse {
                payload: InstallApplicationResponse::new(application_id),
            }
            .into_response()
        }
        Err(err) => {
            error!(path=%req.path, error=?err, "Failed to install dev application");
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
        }
    }
}
