use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{
    ClaimGroupInvitationApiRequest, ClaimGroupInvitationApiResponse,
    ClaimGroupInvitationApiResponseData,
};
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<ClaimGroupInvitationApiRequest>,
) -> impl IntoResponse {
    info!("Claiming group invitation (applying governance op from joiner)");

    let op_bytes = match hex::decode(&req.governance_op) {
        Ok(bytes) => bytes,
        Err(err) => {
            error!(error=?err, "Failed to decode governance_op hex");
            return parse_api_error(eyre::eyre!("invalid governance_op hex: {err}"))
                .into_response();
        }
    };

    let op: calimero_context_primitives::local_governance::SignedGroupOp =
        match borsh::from_slice(&op_bytes) {
            Ok(op) => op,
            Err(err) => {
                error!(error=?err, "Failed to deserialize governance op");
                return parse_api_error(eyre::eyre!("invalid governance op: {err}"))
                    .into_response();
            }
        };

    let result = state
        .ctx_client
        .apply_signed_group_op(op)
        .await
        .map_err(parse_api_error);

    match result {
        Ok(_) => {
            info!("Group invitation claimed successfully");
            ApiResponse {
                payload: ClaimGroupInvitationApiResponse {
                    data: ClaimGroupInvitationApiResponseData { success: true },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(error=?err, "Failed to claim group invitation");
            err.into_response()
        }
    }
}
