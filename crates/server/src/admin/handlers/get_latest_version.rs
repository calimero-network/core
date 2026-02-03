use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::GetLatestVersionResponse;
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Path(package): Path<String>,
) -> impl IntoResponse {
    info!(package=%package, "Getting latest version for package");

    let latest_version = state
        .node_client
        .get_latest_version(&package)
        .map_err(|err| parse_api_error(err).into_response());
    match latest_version {
        Ok(Some((version, application_id))) => {
            info!(package=%package, %version, application_id=%application_id, "Latest version retrieved successfully");
            ApiResponse {
                payload: GetLatestVersionResponse::new(
                    Some(application_id),
                    Some(version),
                ),
            }
            .into_response()
        }
        Ok(None) => {
            info!(package=%package, "No versions found for package");
            ApiResponse {
                payload: GetLatestVersionResponse::new(None, None),
            }
            .into_response()
        }
        Err(err) => {
            error!(package=%package, "Failed to get latest version");
            err.into_response()
        }
    }
}
