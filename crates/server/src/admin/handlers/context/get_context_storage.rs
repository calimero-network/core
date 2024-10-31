use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::GetContextStorageResponse;

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Path(_context_id): Path<String>,
    Extension(_state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    ApiResponse {
        payload: GetContextStorageResponse::new(0),
    }
    .into_response()
}
