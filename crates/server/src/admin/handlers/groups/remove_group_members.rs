use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::RemoveGroupMembersRequest;
use calimero_server_primitives::admin::RemoveGroupMembersApiRequest;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse, Empty};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    ValidatedJson(req): ValidatedJson<RemoveGroupMembersApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, count=%req.members.len(), "Removing group members");

    // Prefer the authenticated identity over the caller-supplied requester to
    // prevent authorization bypass via a spoofed public key in the request body.
    let requester = auth_key.map(|Extension(k)| k.0).or(req.requester);

    let result = state
        .ctx_client
        .remove_group_members(RemoveGroupMembersRequest {
            group_id,
            members: req.members,
            requester,
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
