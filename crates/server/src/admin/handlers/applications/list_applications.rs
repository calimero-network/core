use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::ListApplicationsResponse;
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    info!("Listing applications");

    let applications = state
        .node_client
        .list_applications()
        .map_err(|err| parse_api_error(err).into_response());
    match applications {
        Ok(applications) => {
            info!(count=%applications.len(), "Applications listed successfully");
            ApiResponse {
                payload: ListApplicationsResponse::new(applications),
            }
            .into_response()
        }
        Err(err) => {
            error!("Failed to list applications");
            err.into_response()
        }
    }
}
