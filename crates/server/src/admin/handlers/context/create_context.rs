use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{CreateContextRequest, CreateContextResponse};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<CreateContextRequest>,
) -> impl IntoResponse {
    let result = state
        .ctx_manager
        .create_context(
            req.context_seed.map(Into::into),
            req.application_id,
            None,
            req.initialization_params,
        )
        .await
        .map_err(parse_api_error);

    match result {
        Ok((context_id, member_public_key)) => ApiResponse {
            payload: CreateContextResponse::new(context_id, member_public_key),
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
