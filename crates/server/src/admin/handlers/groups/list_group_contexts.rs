use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::ListGroupContextsRequest;
use calimero_server_primitives::admin::{
    GroupContextEntryResponse, ListGroupContextsApiResponse, ListGroupContextsQuery,
};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Query(query): Query<ListGroupContextsQuery>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(100);

    info!(group_id=%group_id_str, %offset, %limit, "Listing group contexts");

    let result = state
        .ctx_client
        .list_group_contexts(ListGroupContextsRequest {
            group_id,
            offset,
            limit,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(entries) => {
            info!(group_id=%group_id_str, count=%entries.len(), "Group contexts retrieved successfully");
            let data = entries
                .into_iter()
                .map(|e| GroupContextEntryResponse {
                    context_id: hex::encode(*e.context_id),
                    alias: e.alias,
                })
                .collect();
            ApiResponse {
                payload: ListGroupContextsApiResponse { data },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to list group contexts");
            err.into_response()
        }
    }
}
