use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::GenerateContextIdentityResponse;
use calimero_store::types::ContextIdentity as ContextIdentityValue;
use tracing::error;

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

const PLACEHOLDER_CONTEXT_ID_BYTES: [u8; 32] = [0; 32];

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    let private_key = state.ctx_manager.new_private_key();
    let public_key = private_key.public_key();

    let store_result = (|| {
        let placeholder_context_id = ContextId::from(PLACEHOLDER_CONTEXT_ID_BYTES);
        let value = ContextIdentityValue {
            private_key: Some(*private_key),
            sender_key: None,
        };
        state
            .ctx_manager
            .store_identity_value(placeholder_context_id, public_key, value)?;
        Ok::<(), eyre::Report>(())
    })();

    match store_result {
        Ok(_) => ApiResponse {
            payload: GenerateContextIdentityResponse::new(public_key),
        }
        .into_response(),
        Err(e) => {
            error!("Failed to store temporary context identity key: {}", e);
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to generate context identity".into(),
            }
            .into_response()
        }
    }
}
