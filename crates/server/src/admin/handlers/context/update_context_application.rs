use core::str::FromStr;
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{
    UpdateContextApplicationRequest, UpdateContextApplicationResponse,
};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Path(context_id): Path<String>,
    ValidatedJson(req): ValidatedJson<UpdateContextApplicationRequest>,
) -> impl IntoResponse {
    let context_id_result = match ContextId::from_str(&context_id) {
        Ok(id) => id,
        Err(err) => {
            error!(context_id=%context_id, error=?err, "Invalid context ID format");
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid context id".into(),
            }
            .into_response();
        }
    };

    info!(context_id=%context_id_result, application_id=%req.application_id, "Updating context application");

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
        Ok(()) => {
            info!(context_id=%context_id_result, application_id=%req.application_id, "Context application updated successfully");
            ApiResponse {
                payload: UpdateContextApplicationResponse::new(),
            }
            .into_response()
        }
        Err(err) => {
            error!(context_id=%context_id_result, application_id=%req.application_id, error=?err, "Failed to update context application");
            err.into_response()
        }
    }
}
