use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{
    JoinContextRequest, JoinContextResponse, JoinContextResponseData,
};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(JoinContextRequest {
        private_key,
        invitation_payload,
        ..
    }): Json<JoinContextRequest>,
) -> impl IntoResponse {
    let result = state
        .ctx_manager
        .join_context(private_key, invitation_payload)
        .await
        .map_err(parse_api_error);

    match result {
        Ok(r) => ApiResponse {
            payload: JoinContextResponse::new(r.map(|(context_id, member_public_key)| {
                JoinContextResponseData::new(context_id, member_public_key)
            })),
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
