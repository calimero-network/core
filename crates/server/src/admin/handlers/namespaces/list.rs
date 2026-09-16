use std::sync::Arc;

use axum::extract::Query;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::ListNamespacesRequest;
use calimero_context_config::types::ContextGroupId;
use calimero_server_primitives::admin::{
    ListNamespacesApiResponse, ListNamespacesQuery, NamespaceApiResponse,
};
use tracing::{error, info};

use crate::admin::caller_scope::{list_scope_for, ListScope};
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::{AuthenticatedAccount, AuthenticatedNodeOwner};
use crate::AdminState;

pub async fn handler(
    Query(query): Query<ListNamespacesQuery>,
    Extension(state): Extension<Arc<AdminState>>,
    node_owner: Option<Extension<AuthenticatedNodeOwner>>,
    account: Option<Extension<AuthenticatedAccount>>,
) -> impl IntoResponse {
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(100);

    let scope = match list_scope_for(&state.ctx_client, node_owner, account) {
        Ok(scope) => scope,
        Err(err) => {
            error!(error=?err, "Failed to resolve the caller's list scope");
            return parse_api_error(err).into_response();
        }
    };

    info!(%offset, %limit, account = ?scope.account(), "Listing namespaces");

    // An account scope pages the CALLER's namespaces, so the filter has to run
    // before `offset` is applied — hence the unpaginated fetch and the manual
    // slice below. Filtering a page would let an offset past the caller's own
    // count return an empty list while the node holds plenty, with nothing to
    // say why.
    //
    // Unpaginated is affordable here in a way it would not be for contexts: a
    // namespace is a root group, so the count tracks the number of tenants on
    // the node, not their activity. `/contexts` takes the other route and
    // enumerates from the caller's groups instead.
    let request = match scope {
        ListScope::NodeWide => ListNamespacesRequest { offset, limit },
        ListScope::Account { .. } => ListNamespacesRequest {
            offset: 0,
            limit: usize::MAX,
        },
    };

    let result = state
        .ctx_client
        .list_namespaces(request)
        .await
        .map_err(parse_api_error);

    match result {
        Ok(entries) => {
            // A namespace IS a root group, so its id is the group id the scope
            // is expressed in.
            let entries: Vec<_> = match &scope {
                ListScope::NodeWide => entries,
                ListScope::Account { .. } => entries
                    .into_iter()
                    .filter(|ns| {
                        scope.admits(Some(&ContextGroupId::from(ns.namespace_id.to_bytes())))
                    })
                    .skip(offset)
                    .take(limit)
                    .collect(),
            };
            let mut data = Vec::with_capacity(entries.len());
            // Namespaces routinely share an bytecode_id; resolve each blob's
            // manifest version once per request instead of once per row.
            let mut version_memo: std::collections::HashMap<[u8; 32], Option<String>> =
                std::collections::HashMap::new();
            for ns in entries {
                let bytecode_id = ns.bytecode_id.to_bytes();
                let app_version = match version_memo.get(&bytecode_id) {
                    Some(v) => v.clone(),
                    None => {
                        let v = super::namespace_app_version(&state.node_client, bytecode_id).await;
                        let _ = version_memo.insert(bytecode_id, v.clone());
                        v
                    }
                };
                data.push(NamespaceApiResponse {
                    namespace_id: hex::encode(ns.namespace_id.to_bytes()),
                    bytecode_id: hex::encode(ns.bytecode_id.to_bytes()),
                    target_application_id: ns.target_application_id.to_string(),
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
