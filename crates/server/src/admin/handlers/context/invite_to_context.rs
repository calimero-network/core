use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{InviteToContextRequest, InviteToContextResponse};
use reqwest::StatusCode;
use tokio::task;

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<InviteToContextRequest>,
) -> impl IntoResponse {
    let state_clone = Arc::clone(&state);

    // Use spawn_blocking to isolate async code with references
    let result = task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            state_clone
                .ctx_client
                .invite_member(&req.context_id, &req.inviter_id, &req.invitee_id)
                .await
        })
    })
    .await;

    match result {
        Ok(Ok(invitation_payload)) => ApiResponse {
            payload: InviteToContextResponse::new(invitation_payload),
        }
        .into_response(),
        Ok(Err(err)) => parse_api_error(err).into_response(),
        Err(_) => ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Failed to invite member".into(),
        }
        .into_response(),
    }
}
