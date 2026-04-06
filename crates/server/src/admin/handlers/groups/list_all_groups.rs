use std::sync::Arc;

use axum::extract::Query;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::ListAllGroupsRequest;
use calimero_server_primitives::admin::{
    GroupSummaryApiData, ListAllGroupsApiResponse, ListAllGroupsQuery,
};
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Query(query): Query<ListAllGroupsQuery>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(100);

    info!(%offset, %limit, "Listing all groups");

    let result = state
        .ctx_client
        .list_all_groups(ListAllGroupsRequest { offset, limit })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(groups) => {
            info!(count=%groups.len(), "Groups retrieved successfully");
            let data = groups
                .into_iter()
                .map(|g| GroupSummaryApiData {
                    group_id: hex::encode(g.group_id.to_bytes()),
                    app_key: hex::encode(g.app_key.to_bytes()),
                    target_application_id: g.target_application_id,
                    upgrade_policy: g.upgrade_policy,
                    created_at: g.created_at,
                    alias: g.alias,
                })
                .collect();
            ApiResponse {
                payload: ListAllGroupsApiResponse { data },
            }
            .into_response()
        }
        Err(err) => {
            error!(error=?err, "Failed to list groups");
            err.into_response()
        }
    }
}
