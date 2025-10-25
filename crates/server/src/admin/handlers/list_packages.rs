use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::ListPackagesResponse;
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    info!("Listing packages");

    let packages = state
        .node_client
        .list_packages()
        .map_err(|err| parse_api_error(err).into_response());
    match packages {
        Ok(packages) => {
            info!(count=%packages.len(), "Packages listed successfully");
            ApiResponse {
                payload: ListPackagesResponse::new(packages),
            }
            .into_response()
        }
        Err(err) => {
            error!("Failed to list packages");
            err.into_response()
        }
    }
}
