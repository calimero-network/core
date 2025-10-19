use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::GenerateContextIdentityResponse;
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    info!("Generating context identity");

    match state.ctx_client.new_identity() {
        Ok(public_key) => {
            info!(public_key=%public_key, "Context identity generated successfully");
            ApiResponse {
                payload: GenerateContextIdentityResponse::new(public_key),
            }
            .into_response()
        }
        Err(e) => {
            error!(error=?e, "Failed to generate context identity");
            parse_api_error(e).into_response()
        }
    }
}
