use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{JoinContextRequest, JoinContextResponse};
use tracing::error;

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(JoinContextRequest { invitation_payload }): Json<JoinContextRequest>,
) -> impl IntoResponse {
    let invitee_id_result = invitation_payload.parts().map(|(_, id, _, _, _)| id);
    let invitee_public_key = match invitee_id_result {
        Ok(id) => id,
        Err(e) => {
            error!("Failed to parse invitation payload: {}", e);
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid invitation payload".into(),
            }
            .into_response();
        }
    };

    let result = state
        .ctx_manager
        .join_context(invitee_public_key, invitation_payload)
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
