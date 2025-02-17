use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{CreateIdentityAliasRequest, CreateIdentityAliasResponse};
use reqwest::StatusCode;

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(payload): Json<CreateIdentityAliasRequest>,
) -> impl IntoResponse {
    let mut store = state.store.handle();

    if let Ok(key) =
        state
            .ctx_manager
            .create_key_from_alias(payload.kind, payload.alias, payload.context_id)
    {
        return match store.put(&key, &payload.hash) {
            Ok(_) => ApiResponse {
                payload: CreateIdentityAliasResponse::new(),
            }
            .into_response(),
            Err(err) => ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to store alias: {}", err),
            }
            .into_response(),
        };
    } else {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "context_id is required for identity aliases".into(),
        }
        .into_response();
    }
}
