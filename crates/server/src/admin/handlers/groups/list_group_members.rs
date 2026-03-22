use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::ListGroupMembersRequest;
use calimero_server_primitives::admin::{
    GroupMemberApiEntry, ListGroupMembersApiResponse, ListGroupMembersQuery,
};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Query(query): Query<ListGroupMembersQuery>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(100);

    info!(group_id=%group_id_str, %offset, %limit, "Listing group members");

    let result = state
        .ctx_client
        .list_group_members(ListGroupMembersRequest {
            group_id,
            offset,
            limit,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(group_id=%group_id_str, count=%resp.members.len(), "Group members retrieved successfully");
            let entries = resp
                .members
                .into_iter()
                .map(|m| GroupMemberApiEntry {
                    identity: m.identity,
                    role: m.role,
                    alias: m.alias,
                })
                .collect();
            ApiResponse {
                payload: ListGroupMembersApiResponse {
                    data: entries,
                    self_identity: Some(resp.self_identity),
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to list group members");
            err.into_response()
        }
    }
}
