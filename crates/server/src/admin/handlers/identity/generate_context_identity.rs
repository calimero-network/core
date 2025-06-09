use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::GenerateContextIdentityResponse;
use tracing::error;

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    let private_key = state.ctx_client.new_private_key();

    ApiResponse {
        payload: GenerateContextIdentityResponse::new(private_key.public_key(), private_key),
    }
    .into_response()
}
