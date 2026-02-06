use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::common::DIGEST_SIZE;
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
    // TODO: figure out the best place to generate salt.
    // We temporarily ignore the passed `secret_salt` as we can't generate it in admin
    // `invite_to_context_open_invitation::handler` as `Rng` is not thread-safe.
    //let mut rng = rand::thread_rng();
    //let salt: [u8; DIGEST_SIZE] = rng.gen::<[_; DIGEST_SIZE]>();
    let salt = [0u8; DIGEST_SIZE];

    let result = state
        .ctx_client
        .invite_member_by_open_invitation(
            &req.context_id,
            &req.inviter_id,
            req.valid_for_blocks,
            salt,
        )
        .await;

    match result {
        Ok(signed_open_invitation) => ApiResponse {
            payload: InviteToContextOpenInvitationResponse::new(signed_open_invitation),
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}
