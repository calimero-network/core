use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::ListVersionsResponse;
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Path(package): Path<String>,
) -> impl IntoResponse {
    info!(package=%package, "Listing versions for package");

    let versions = state
        .node_client
        .list_versions(&package)
        .map_err(|err| parse_api_error(err).into_response());
    match versions {
        Ok(versions) => {
            info!(package=%package, count=%versions.len(), "Versions listed successfully");
            ApiResponse {
                payload: ListVersionsResponse::new(versions),
            }
            .into_response()
        }
        Err(err) => {
            error!(package=%package, "Failed to list versions");
            err.into_response()
        }
    }
}
