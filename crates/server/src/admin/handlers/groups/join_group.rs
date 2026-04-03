use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::JoinGroupRequest;
use calimero_server_primitives::admin::{
    JoinGroupApiRequest, JoinGroupApiResponse, JoinGroupApiResponseData,
};
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<JoinGroupApiRequest>,
) -> impl IntoResponse {
    info!("Joining group via invitation");

    let result = state
        .ctx_client
        .join_group(JoinGroupRequest {
            invitation: req.invitation,
            group_alias: req.group_alias,
        })
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
                        governance_op: hex::encode(&resp.governance_op_bytes),
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
