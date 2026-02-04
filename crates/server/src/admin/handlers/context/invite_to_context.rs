use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{InviteToContextRequest, InviteToContextResponse};
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<InviteToContextRequest>,
) -> impl IntoResponse {
    info!(context_id=%req.context_id, invitee_id=%req.invitee_id, "Inviting member to context");

    let result = state
        .ctx_client
        .invite_member(&req.context_id, &req.inviter_id, &req.invitee_id)
        .await;

    match result {
        Ok(invitation_payload) => {
            info!(context_id=%req.context_id, invitee_id=%req.invitee_id, "Invitation created successfully");
            ApiResponse {
                payload: InviteToContextResponse::new(invitation_payload),
            }
            .into_response()
        }
        Err(err) => {
            error!(context_id=%req.context_id, invitee_id=%req.invitee_id, error=?err, "Failed to create invitation");
            parse_api_error(err).into_response()
        }
    }
}
