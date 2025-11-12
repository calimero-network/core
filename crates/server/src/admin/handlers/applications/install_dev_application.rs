use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{InstallApplicationResponse, InstallDevApplicationRequest};
use tracing::{debug, error, info};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<InstallDevApplicationRequest>,
) -> impl IntoResponse {
    info!(path=%req.path, "Installing dev application");
    let metadata_len = req.metadata.len();
    debug!(
        path=%req.path,
        metadata_len,
        package = req.package.as_deref().unwrap_or("unknown"),
        version = req.version.as_deref().unwrap_or("0.0.0"),
        "install_dev_application request received"
    );

    match state
        .node_client
        .install_application_from_path(req.path.clone(), req.metadata)
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
            error!(
                path=%req.path,
                package = req.package.as_deref().unwrap_or("unknown"),
                version = req.version.as_deref().unwrap_or("0.0.0"),
                error = ?err,
                "Failed to install dev application"
            );
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
        }
    }
}
