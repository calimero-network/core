use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{InviteToContextRequest, InviteToContextResponse};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<InviteToContextRequest>,
) -> impl IntoResponse {
    let result = state
        .ctx_client
        .invite_member(&req.context_id, &req.inviter_id, &req.invitee_id)
        .await;

    match result {
        Ok(invitation_payload) => ApiResponse {
            payload: InviteToContextResponse::new(invitation_payload),
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}
