use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::GenerateContextIdentityResponse;
use tracing::error;

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    match state.ctx_manager.pre_store_new_identity() {
        Ok(public_key) => ApiResponse {
            payload: GenerateContextIdentityResponse::new(public_key),
        }
        .into_response(),
        Err(e) => {
            error!("Failed to generate and pre-store context identity: {}", e);
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to generate context identity".into(),
            }
            .into_response()
        }
    }
}
