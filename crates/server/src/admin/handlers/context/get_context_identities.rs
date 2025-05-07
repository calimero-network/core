use std::sync::Arc;

use axum::extract::{Path, Request};
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::GetContextIdentitiesResponse;
use reqwest::StatusCode;

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
    req: Request,
) -> impl IntoResponse {
    let context = state
        .ctx_client
        .get_context(&context_id)
        .map_err(|err| parse_api_error(err).into_response());

    let context = match context {
        Ok(context) => context,
        Err(err) => return err.into_response(),
    };

    let context = match context {
        Some(context) => context,
        None => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Context not found".into(),
            }
            .into_response()
        }
    };

    let owned = req.uri().path().ends_with("identities-owned");

    let context_identities = state
        .ctx_client
        .context_members(&context_id, Some(owned))
        .map_err(|err| parse_api_error(err).into_response());

    match context_identities {
        Ok(identities) => ApiResponse {
            payload: GetContextIdentitiesResponse::new(identities),
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
