use core::str::FromStr;
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{
    UpdateContextApplicationRequest, UpdateContextApplicationResponse,
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
        .ctx_client
        .update_application(
            &context_id_result,
            &req.application_id,
            &req.executor_public_key,
        )
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => ApiResponse {
            payload: UpdateContextApplicationResponse::new(),
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
