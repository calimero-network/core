use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::ListApplicationsResponse;

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    let applications = state
        .node_client
        .list_applications()
        .map_err(|err| parse_api_error(err).into_response());
    match applications {
        Ok(applications) => {
            ApiResponse {
                payload: ListApplicationsResponse::new(applications),
            }
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
