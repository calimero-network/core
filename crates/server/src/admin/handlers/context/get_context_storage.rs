use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::GetContextStorageResponse;
use tracing::info;

use crate::admin::handlers::usage::context_storage_bytes;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(context_id): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let context_id: ContextId = match context_id.parse() {
        Ok(id) => id,
        Err(_) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid context ID format".to_owned(),
            }
            .into_response();
        }
    };

    let size_in_bytes = context_storage_bytes(&state.store, context_id.as_ref());
    info!(context_id=%context_id, size_in_bytes, "Reporting context storage");

    ApiResponse {
        payload: GetContextStorageResponse::new(size_in_bytes),
    }
    .into_response()
}
