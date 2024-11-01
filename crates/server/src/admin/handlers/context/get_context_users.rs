use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{GetContextUsersResponse, GetContextUsersResponseData};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Path(_context_id): Path<String>,
    Extension(_state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    ApiResponse {
        payload: GetContextUsersResponse {
            data: GetContextUsersResponseData {
                context_users: vec![],
            },
        },
    }
    .into_response()
}
