use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::TeeInfoResponse;
use calimero_tee_attestation::get_tee_info;
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(Extension(_state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    info!("Getting TEE info");

    match get_tee_info().await {
        Ok(tee_info) => {
            info!(
                cloud_provider=%tee_info.cloud_provider,
                os_image=%tee_info.os_image,
                "TEE info retrieved successfully"
            );

            ApiResponse {
                payload: TeeInfoResponse::new(
                    tee_info.cloud_provider,
                    tee_info.os_image,
                    tee_info.mrtd,
                ),
            }
            .into_response()
        }
        Err(err) => {
            error!(error=%err, "Failed to get TEE info");

            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to get TEE info: {}", err),
            }
            .into_response()
        }
    }
}
