use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::alias::Kind;
use calimero_server_primitives::admin::{CreateIdentityAliasRequest, CreateIdentityAliasResponse};
use calimero_store::key::IdentityAlias;
use reqwest::StatusCode;

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(payload): Json<CreateIdentityAliasRequest>,
) -> impl IntoResponse {
    let mut store = state.store.handle();

    let key = match payload.kind {
        Kind::Context => IdentityAlias::context(payload.alias),
        Kind::Identity => match payload.context_id {
            Some(context_id) => IdentityAlias::identity(context_id, payload.alias),
            None => {
                return ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "context_id is required for identity aliases".into(),
                }
                .into_response()
            }
        },
        Kind::Application => IdentityAlias::application(payload.alias),
    };

    match store.put(&key, &payload.hash) {
        Ok(_) => ApiResponse {
            payload: CreateIdentityAliasResponse::new(),
        }
        .into_response(),
        Err(err) => ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("Failed to store alias: {}", err),
        }
        .into_response(),
    }
}
