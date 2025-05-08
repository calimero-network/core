use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::CreateContextRequest;

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<CreateContextRequest>,
) -> impl IntoResponse {
    let result = state
        .ctx_client
        .create_context(
            req.protocol,
            &req.application_id,
            None,
            req.initialization_params,
            req.context_seed.map(Into::into),
        )
        .await
        .map_err(parse_api_error);

    match result {
        Ok(response) => ApiResponse { payload: response }.into_response(),
        Err(err) => err.into_response(),
    }
}
