use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::GenerateContextIdentityResponse;

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    match state.ctx_client.new_identity(None) {
        Ok(public_key) => ApiResponse {
            payload: GenerateContextIdentityResponse::new(public_key),
        }
        .into_response(),
        Err(e) => parse_api_error(e).into_response(),
    }
}
