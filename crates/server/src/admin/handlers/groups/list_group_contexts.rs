use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::ListGroupContextsRequest;
use calimero_server_primitives::admin::{
    GroupContextEntryResponse, ListGroupContextsApiResponse, ListGroupContextsQuery,
};
use tracing::{error, info};

use super::{parse_group_id, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT};
use crate::admin::service::{parse_api_error, ApiResponse};

use crate::admin::caller_scope::list_scope_for;
use crate::auth::{AuthenticatedAccount, AuthenticatedDevice, AuthenticatedNodeOwner};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Query(query): Query<ListGroupContextsQuery>,
    Extension(state): Extension<Arc<AdminState>>,
    node_owner: Option<Extension<AuthenticatedNodeOwner>>,
    account: Option<Extension<AuthenticatedAccount>>,
    device: Option<Extension<AuthenticatedDevice>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    // Before any read, so a refusal costs one membership lookup rather than the
    // group load below.
    let scope = match list_scope_for(&state.ctx_client, node_owner, account, device) {
        Ok(scope) => scope,
        Err(err) => {
            error!(error=?err, "Failed to resolve the caller's list scope");
            return parse_api_error(err).into_response();
        }
    };
    if let Some(refusal) =
        crate::admin::caller_scope::refuse_unless_group_in_scope(&scope, &group_id)
    {
        return refusal;
    }

    let offset = query.offset.unwrap_or(0);
    let limit = query
        .limit
        .unwrap_or(DEFAULT_LIST_LIMIT)
        .min(MAX_LIST_LIMIT);

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
                    context_id: e.context_id.to_string(),
                    name: e.name,
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
