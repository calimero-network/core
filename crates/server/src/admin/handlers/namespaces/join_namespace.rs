use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::JoinGroupRequest;
use calimero_server_primitives::admin::{
    JoinGroupApiRequest, JoinGroupApiResponse, JoinGroupApiResponseData,
};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{ApiError, parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<JoinGroupApiRequest>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let invitation_group_id = req.invitation.invitation.group_id;
    if invitation_group_id != namespace_id {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "invitation group_id does not match namespace_id in path".into(),
        }
        .into_response();
    }

    info!(namespace_id=%namespace_id_str, "Joining namespace via invitation");

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
            info!(group_id=%group_id_hex, member=%resp.member_identity, "Joined namespace successfully");
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
            error!(namespace_id=%namespace_id_str, error=?err, "Failed to join namespace");
            err.into_response()
        }
    }
}
