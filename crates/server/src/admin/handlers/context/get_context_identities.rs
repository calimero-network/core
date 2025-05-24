use std::sync::Arc;

use axum::extract::{Path, Request};
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::GetContextIdentitiesResponse;
use futures_util::TryStreamExt;
use reqwest::StatusCode;

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
    req: Request,
) -> impl IntoResponse {
    let owned = req.uri().path().ends_with("identities-owned");

    let context = match state.ctx_client.get_context(&context_id) {
        Ok(Some(context)) => context,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Context not found".into(),
            }
            .into_response()
        }
        Err(err) => return parse_api_error(err).into_response(),
    };

    let stream = state.ctx_client.context_members(&context.id, Some(owned));

    match stream.map_ok(|(id, _)| id).try_collect().await {
        Ok(identities) => ApiResponse {
            payload: GetContextIdentitiesResponse::new(identities),
        }
        .into_response(),
        Err(_) => ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Failed to process identities".into(),
        }
        .into_response(),
    }
}
