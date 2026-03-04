use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::UpdateMemberRoleRequest;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::UpdateMemberRoleApiRequest;
use reqwest::StatusCode;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse, Empty};
use crate::AdminState;

pub async fn handler(
    Path((group_id_str, identity_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<UpdateMemberRoleApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let identity = match parse_identity(&identity_str) {
        Ok(pk) => pk,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, identity=%identity_str, "Updating member role");

    let result = state
        .ctx_client
        .update_member_role(UpdateMemberRoleRequest {
            group_id,
            identity,
            new_role: req.role,
            requester: req.requester,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, identity=%identity_str, "Member role updated successfully");
            ApiResponse { payload: Empty }.into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, identity=%identity_str, error=?err, "Failed to update member role");
            err.into_response()
        }
    }
}

fn parse_identity(s: &str) -> Result<PublicKey, ApiError> {
    let bytes = hex::decode(s).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid identity format: expected hex-encoded 32 bytes".into(),
    })?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid identity: must be exactly 32 bytes".into(),
    })?;
    Ok(PublicKey::from(arr))
}
