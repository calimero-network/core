use core::str::FromStr;
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{
    Empty, UpdateContextApplicationRequest, UpdateContextApplicationResponse,
};
use reqwest::StatusCode;

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Path(context_id): Path<String>,
    Json(req): Json<UpdateContextApplicationRequest>,
) -> impl IntoResponse {
    let Ok(context_id_result) = ContextId::from_str(&context_id) else {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Invalid context id".into(),
        }
        .into_response();
    };

    let result = state
        .ctx_manager
        .update_application_id(context_id_result, req.application_id)
        .map_err(parse_api_error);

    match result {
        Ok(()) => ApiResponse {
            payload: UpdateContextApplicationResponse { data: Empty {} },
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
