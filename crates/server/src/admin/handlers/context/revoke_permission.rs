use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{RevokePermissionRequest, RevokePermissionResponse};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<RevokePermissionRequest>,
) -> impl IntoResponse {
    let result = state
        .ctx_client
        .revoke_permission(
            req.context_id,
            req.revoker_id,
            req.revokee_id,
            req.capability,
        )
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => ApiResponse {
            payload: RevokePermissionResponse::new(),
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
