use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_config::VisibilityMode;
use calimero_context_primitives::group::SetContextVisibilityRequest;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::SetContextVisibilityApiRequest;
use reqwest::StatusCode;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse, Empty};
use crate::AdminState;

pub async fn handler(
    Path((group_id_str, context_id_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<SetContextVisibilityApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let context_id = match parse_context_id(&context_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let mode = match req.mode.as_str() {
        "open" => VisibilityMode::Open,
        "restricted" => VisibilityMode::Restricted,
        _ => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "mode must be 'open' or 'restricted'".into(),
            }
            .into_response()
        }
    };

    info!(group_id=%group_id_str, context_id=%context_id_str, ?mode, "Setting context visibility");

    let result = state
        .ctx_client
        .set_context_visibility(SetContextVisibilityRequest {
            group_id,
            context_id,
            mode,
            requester: req.requester,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, context_id=%context_id_str, "Context visibility updated");
            ApiResponse { payload: Empty }.into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, context_id=%context_id_str, error=?err, "Failed to set context visibility");
            err.into_response()
        }
    }
}

fn parse_context_id(s: &str) -> Result<ContextId, ApiError> {
    let bytes = hex::decode(s).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid context id format: expected hex-encoded 32 bytes".into(),
    })?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid context id: must be exactly 32 bytes".into(),
    })?;
    Ok(ContextId::from(arr))
}
