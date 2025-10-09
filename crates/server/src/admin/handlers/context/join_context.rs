use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{JoinContextRequest, JoinContextResponse};
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(JoinContextRequest { invitation_payload }): Json<JoinContextRequest>,
) -> impl IntoResponse {
    info!("Joining context");

    let result = state
        .ctx_client
        .join_context(invitation_payload)
        .await
        .map_err(parse_api_error);

    match result {
        Ok(result) => {
            info!(context_id=%result.context_id, "Joined context successfully");
            ApiResponse {
                payload: JoinContextResponse::new(Some((
                    result.context_id,
                    result.member_public_key,
                ))),
            }
            .into_response()
        }
        Err(err) => {
            error!(error=?err, "Failed to join context");
            err.into_response()
        }
    }
}
