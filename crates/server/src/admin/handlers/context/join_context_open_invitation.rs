use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{JoinContextByOpenInvitationRequest, JoinContextResponse};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(JoinContextByOpenInvitationRequest {
        invitation,
        new_member_public_key,
    }): Json<JoinContextByOpenInvitationRequest>,
) -> impl IntoResponse {
    let result = state
        .ctx_client
        .join_context_by_open_invitation(invitation, &new_member_public_key)
        .await
        .map_err(parse_api_error);

    match result {
        Ok(result) => {
            ApiResponse {
                payload: JoinContextResponse::new(
                    result.map(|r| (r.context_id, r.member_public_key)),
                ),
            }
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
