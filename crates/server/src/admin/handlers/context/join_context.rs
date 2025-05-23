use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{JoinContextRequest, JoinContextResponse};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(JoinContextRequest { invitation_payload }): Json<JoinContextRequest>,
) -> impl IntoResponse {
    let result = state
        .ctx_manager
        .join_context(invitation_payload)
        .await
        .map_err(parse_api_error);

    match result {
        Ok(result) => ApiResponse {
            payload: JoinContextResponse::new(result),
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
