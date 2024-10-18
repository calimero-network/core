use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::ContextStorage;
use serde::Serialize;

use crate::admin::service::ApiResponse;
use crate::AdminState;

#[derive(Debug, Serialize)]
struct GetContextStorageResponse {
    data: ContextStorage,
}

impl GetContextStorageResponse {
    #[must_use]
    pub const fn new(size_in_bytes: u64) -> Self {
        Self {
            data: ContextStorage::new(size_in_bytes),
        }
    }
}

pub async fn handler(
    Path(_context_id): Path<String>,
    Extension(_state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    ApiResponse {
        payload: GetContextStorageResponse::new(0),
    }
    .into_response()
}
