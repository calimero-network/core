use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{
    InviteToContextOpenInvitationRequest, InviteToContextOpenInvitationResponse,
};
//use rand::Rng;

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<InviteToContextOpenInvitationRequest>,
) -> impl IntoResponse {
    let salt = [0u8; 32];

    let result = state
        .ctx_client
        .invite_member_by_open_invitation(
            &req.context_id,
            &req.inviter_id,
            req.valid_for_seconds,
            salt,
        )
        .await;

    match result {
        Ok(ref data) => {
            tracing::info!(
                context_id=%req.context_id,
                inviter_id=%req.inviter_id,
                has_data=data.is_some(),
                "open invitation result"
            );
        }
        Err(ref err) => {
            tracing::error!(
                context_id=%req.context_id,
                inviter_id=%req.inviter_id,
                error=?err,
                "open invitation error"
            );
        }
    }
    match result {
        Ok(signed_open_invitation) => ApiResponse {
            payload: InviteToContextOpenInvitationResponse::new(signed_open_invitation),
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}
