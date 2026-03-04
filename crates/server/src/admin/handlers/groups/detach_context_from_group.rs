use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::DetachContextFromGroupRequest;
use calimero_server_primitives::admin::DetachContextFromGroupApiRequest;
use reqwest::StatusCode;
use tracing::{error, info};

use super::{decode_signing_key, parse_group_id};
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse, Empty};
use crate::AdminState;

pub async fn handler(
    Path((group_id_str, context_id_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<DetachContextFromGroupApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let context_id = match context_id_str.parse() {
        Ok(id) => id,
        Err(_) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid context_id format".into(),
            }
            .into_response()
        }
    };

    let signing_key = match req.requester_secret.as_deref().map(decode_signing_key) {
        Some(Ok(key)) => Some(key),
        Some(Err(err)) => return err.into_response(),
        None => None,
    };

    info!(group_id=%group_id_str, context_id=%context_id_str, "Detaching context from group");

    let result = state
        .ctx_client
        .detach_context_from_group(DetachContextFromGroupRequest {
            group_id,
            context_id,
            requester: req.requester,
            signing_key,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, context_id=%context_id_str, "Context detached from group successfully");
            ApiResponse { payload: Empty }.into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, context_id=%context_id_str, error=?err, "Failed to detach context from group");
            err.into_response()
        }
    }
}
