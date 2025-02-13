use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::alias::Kind;
use calimero_server_primitives::admin::{DeleteIdentityAliasResponse, GetIdentityAliasRequest};
use calimero_store::key::Alias;
use reqwest::StatusCode;

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(payload): Json<GetIdentityAliasRequest>,
) -> impl IntoResponse {
    let mut store = state.store.handle();

    let key = match payload.kind {
        Kind::Context => Alias::context(payload.alias),
        Kind::Identity => match payload.context_id {
            Some(context_id) => Alias::identity(context_id, payload.alias),
            None => {
                return ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "context_id is required for identity aliases".into(),
                }
                .into_response()
            }
        },
        Kind::Application => Alias::application(payload.alias),
    };

    match store.get(&key) {
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Alias not found".into(),
            }
            .into_response()
        }
        Err(err) => {
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to check alias existence: {}", err),
            }
            .into_response()
        }
        Ok(Some(_)) => {}
    }

    match store.delete(&key) {
        Ok(_) => ApiResponse {
            payload: DeleteIdentityAliasResponse::new(),
        }
        .into_response(),
        Err(err) => ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("Failed to delete alias: {}", err),
        }
        .into_response(),
    }
}
