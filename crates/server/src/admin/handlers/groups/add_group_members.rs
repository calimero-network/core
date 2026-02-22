use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::AddGroupMembersRequest;
use calimero_server_primitives::admin::AddGroupMembersApiRequest;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse, Empty};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<AddGroupMembersApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, count=%req.members.len(), "Adding group members");

    let members = req
        .members
        .into_iter()
        .map(|m| (m.identity, m.role))
        .collect();

    let result = state
        .ctx_client
        .add_group_members(AddGroupMembersRequest {
            group_id,
            members,
            requester: req.requester,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, "Group members added successfully");
            ApiResponse { payload: Empty }.into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to add group members");
            err.into_response()
        }
    }
}
