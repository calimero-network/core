use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::RemoveGroupMembersRequest;
use calimero_server_primitives::admin::RemoveGroupMembersApiRequest;
use tracing::{error, info};

use super::{decode_signing_key, parse_group_id};
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse, Empty};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<RemoveGroupMembersApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let signing_key = match req.requester_secret.as_deref().map(decode_signing_key) {
        Some(Ok(key)) => Some(key),
        Some(Err(err)) => return err.into_response(),
        None => None,
    };

    info!(group_id=%group_id_str, count=%req.members.len(), "Removing group members");

    let result = state
        .ctx_client
        .remove_group_members(RemoveGroupMembersRequest {
            group_id,
            members: req.members,
            requester: req.requester,
            signing_key,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, "Group members removed successfully");
            ApiResponse { payload: Empty }.into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to remove group members");
            err.into_response()
        }
    }
}
