use std::sync::Arc;

use axum::extract::Query;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::ListNamespacesRequest;
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
            let mut data = Vec::with_capacity(entries.len());
            // Namespaces routinely share an app_key; resolve each blob's
            // manifest version once per request instead of once per row.
            let mut version_memo: std::collections::HashMap<[u8; 32], Option<String>> =
                std::collections::HashMap::new();
            for ns in entries {
                let app_key = ns.app_key.to_bytes();
                let app_version = match version_memo.get(&app_key) {
                    Some(v) => v.clone(),
                    None => {
                        let v = super::namespace_app_version(&state.node_client, app_key).await;
                        let _ = version_memo.insert(app_key, v.clone());
                        v
                    }
                };
                data.push(NamespaceApiResponse {
                    namespace_id: hex::encode(ns.namespace_id.to_bytes()),
                    app_key: hex::encode(ns.app_key.to_bytes()),
                    target_application_id: ns.target_application_id.to_string(),
                    upgrade_policy: format!("{:?}", ns.upgrade_policy),
                    created_at: ns.created_at,
                    name: ns.name,
                    member_count: ns.member_count,
                    context_count: ns.context_count,
                    subgroup_count: ns.subgroup_count,
                    app_version,
                });
            }
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
