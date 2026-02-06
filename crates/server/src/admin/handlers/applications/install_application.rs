use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{InstallApplicationRequest, InstallApplicationResponse};
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<InstallApplicationRequest>,
) -> impl IntoResponse {
    info!(url=%req.url, "Installing application");

    match state
        .node_client
        .install_application_from_url(req.url.clone(), req.metadata, req.hash.as_ref())
        .await
    {
        Ok(application_id) => {
            info!(application_id=%application_id, "Application installed successfully");
            ApiResponse {
                payload: InstallApplicationResponse::new(application_id),
            }
            .into_response()
        }
        Err(err) => {
            error!(url=%req.url, error=?err, "Failed to install application");
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
        }
    }
}
