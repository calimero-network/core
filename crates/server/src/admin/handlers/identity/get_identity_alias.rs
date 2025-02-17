use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{
    GetIdentityAliasRequest, GetIdentityAliasResponse, GetIdentityAliasResponseData,
};
use reqwest::StatusCode;

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(payload): Json<GetIdentityAliasRequest>,
) -> impl IntoResponse {
    let store = state.store.handle();

    if let Ok(key) =
        state
            .ctx_manager
            .create_key_from_alias(payload.kind, payload.alias, payload.context_id)
    {
        return match store.get(&key) {
            Ok(Some(public_key)) => ApiResponse {
                payload: GetIdentityAliasResponse {
                    data: GetIdentityAliasResponseData::new(public_key),
                },
            }
            .into_response(),
            Ok(None) => ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Alias not found".into(),
            }
            .into_response(),
            Err(err) => ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to retrieve alias: {}", err),
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
