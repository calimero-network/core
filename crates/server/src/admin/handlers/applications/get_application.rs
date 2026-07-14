use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::GetApplicationResponse;
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Path(application_id): Path<ApplicationId>,
) -> impl IntoResponse {
    info!(application_id=%application_id, "Getting application");

    match state.node_client.get_application(&application_id) {
        Ok(Some(application)) => {
            info!(application_id=%application_id, "Application retrieved successfully");
            ApiResponse {
                payload: GetApplicationResponse::new(Some(application)),
            }
            .into_response()
        }
        // A missing application is 404, not a 200 with a null payload.
        Ok(None) => {
            info!(application_id=%application_id, "Application not found");
            ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Application not found".to_owned(),
            }
            .into_response()
        }
        Err(err) => {
            error!(application_id=%application_id, error=?err, "Failed to get application");
            parse_api_error(err).into_response()
        }
    }
}
