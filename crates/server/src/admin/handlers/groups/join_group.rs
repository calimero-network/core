use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::JoinGroupRequest;
use calimero_primitives::context::GroupInvitationPayload;
use calimero_server_primitives::admin::{
    JoinGroupApiRequest, JoinGroupApiResponse, JoinGroupApiResponseData,
};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<JoinGroupApiRequest>,
) -> impl IntoResponse {
    let invitation_payload: GroupInvitationPayload = match req.invitation_payload.parse() {
        Ok(p) => p,
        Err(err) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: format!("Invalid invitation payload: {err}"),
            }
            .into_response();
        }
    };

    info!("Joining group via invitation");

    let result = state
        .ctx_client
        .join_group(JoinGroupRequest { invitation_payload })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            let group_id_hex = hex::encode(resp.group_id.to_bytes());
            info!(group_id=%group_id_hex, member=%resp.member_identity, "Joined group successfully");
            ApiResponse {
                payload: JoinGroupApiResponse {
                    data: JoinGroupApiResponseData {
                        group_id: group_id_hex,
                        member_identity: resp.member_identity,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(error=?err, "Failed to join group");
            err.into_response()
        }
    }
}
