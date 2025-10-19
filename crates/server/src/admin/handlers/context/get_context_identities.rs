use std::sync::Arc;

use axum::extract::{Path, Request};
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::GetContextIdentitiesResponse;
use futures_util::TryStreamExt;
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
    req: Request,
) -> impl IntoResponse {
    let owned = req.uri().path().ends_with("identities-owned");

    info!(context_id=%context_id, owned=%owned, "Getting context identities");

    let context = match state.ctx_client.get_context(&context_id) {
        Ok(Some(context)) => context,
        Ok(None) => {
            info!(context_id=%context_id, "Context not found");
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Context not found".into(),
            }
            .into_response();
        }
        Err(err) => {
            error!(context_id=%context_id, error=?err, "Failed to get context");
            return parse_api_error(err).into_response();
        }
    };

    let stream = state
        .ctx_client
        .get_context_members(&context.id, Some(owned));

    match stream.map_ok(|(id, _)| id).try_collect::<Vec<_>>().await {
        Ok(identities) => {
            info!(context_id=%context_id, count=%identities.len(), "Context identities retrieved successfully");
            ApiResponse {
                payload: GetContextIdentitiesResponse::new(identities),
            }
            .into_response()
        }
        Err(err) => {
            error!(context_id=%context_id, error=?err, "Failed to process identities");
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to process identities".into(),
            }
            .into_response()
        }
    }
}
