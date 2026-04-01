use std::sync::Arc;

use axum::extract::Query;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::ListNamespacesRequest;
use calimero_server_primitives::admin::{
    ListNamespacesApiResponse, ListNamespacesQuery, NamespaceApiResponse,
};
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Query(query): Query<ListNamespacesQuery>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(100);

    info!(%offset, %limit, "Listing namespaces");

    let result = state
        .ctx_client
        .list_namespaces(ListNamespacesRequest { offset, limit })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(entries) => {
            let data = entries
                .into_iter()
                .map(|ns| NamespaceApiResponse {
                    namespace_id: hex::encode(ns.namespace_id.to_bytes()),
                    app_key: hex::encode(ns.app_key.to_bytes()),
                    target_application_id: ns.target_application_id.to_string(),
                    upgrade_policy: format!("{:?}", ns.upgrade_policy),
                    created_at: ns.created_at,
                    alias: ns.alias,
                    member_count: ns.member_count,
                    context_count: ns.context_count,
                    subgroup_count: ns.subgroup_count,
                })
                .collect();
            ApiResponse {
                payload: ListNamespacesApiResponse { data },
            }
            .into_response()
        }
        Err(err) => {
            error!(error=?err, "Failed to list namespaces");
            err.into_response()
        }
    }
}
