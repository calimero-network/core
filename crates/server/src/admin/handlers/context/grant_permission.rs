use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{GrantPermissionRequest, GrantPermissionResponse};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<GrantPermissionRequest>,
) -> impl IntoResponse {
    let result = state
        .ctx_manager
        .grant_permission(
            req.context_id,
            req.granter_id,
            req.grantee_id,
            req.capability,
        )
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => ApiResponse {
            payload: GrantPermissionResponse::new(),
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
