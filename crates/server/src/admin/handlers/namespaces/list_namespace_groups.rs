use calimero_governance_store::{MetadataRepository, NamespaceRepository};
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{
    ListNamespaceGroupsApiResponse, NamespaceGroupEntryApiResponse,
};
use tracing::error;

use axum::response::Response;
use calimero_context_config::types::ContextGroupId;
use reqwest::StatusCode;
use tracing::debug;

use crate::admin::caller_scope::{list_scope_for, ListScope};
use crate::admin::handlers::groups::parse_group_id;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::auth::{AuthenticatedAccount, AuthenticatedDevice, AuthenticatedNodeOwner};
use crate::AdminState;

/// Refuse a namespace this caller is not in, as though it were not there.
///
/// **404, not 403.** A 403 confirms the namespace exists, so a caller with no
/// business knowing that could enumerate the node's tenants one id at a time by
/// reading which refusal came back. The listing endpoints already answer "what
/// you are in"; a single-resource read must not become a way to ask "what else
/// is there".
///
/// A node-wide scope admits everything, which is the answer for the node owner
/// and for a node running with no auth guard at all — narrowing there would
/// empty the endpoint on every default-configured node without protecting
/// anything the proxy is not already deciding.
fn refuse_unless_in_scope(scope: &ListScope, namespace_id: &ContextGroupId) -> Option<Response> {
    if scope.admits(Some(namespace_id)) {
        return None;
    }
    debug!(
        namespace_id = ?namespace_id,
        account = ?scope.account(),
        "refusing a namespace outside the caller's scope",
    );
    Some(
        ApiError {
            status_code: StatusCode::NOT_FOUND,
            message: "Namespace not found".to_owned(),
        }
        .into_response(),
    )
}

pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    node_owner: Option<Extension<AuthenticatedNodeOwner>>,
    account: Option<Extension<AuthenticatedAccount>>,
    device: Option<Extension<AuthenticatedDevice>>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let scope = match list_scope_for(&state.ctx_client, node_owner, account, device) {
        Ok(scope) => scope,
        Err(err) => {
            error!(error=?err, "Failed to resolve the caller's list scope");
            return parse_api_error(err).into_response();
        }
    };
    if let Some(refusal) = refuse_unless_in_scope(&scope, &namespace_id) {
        return refusal;
    }

    let groups = match NamespaceRepository::new(&state.store).list_children(&namespace_id) {
        Ok(groups) => groups,
        Err(err) => return parse_api_error(err).into_response(),
    };

    let mut entries = Vec::with_capacity(groups.len());
    for group_id in groups {
        let name = match MetadataRepository::new(&state.store).group_metadata(&group_id) {
            Ok(rec) => rec.and_then(|r| r.name),
            Err(err) => {
                error!(
                    ?err,
                    "Failed to resolve group metadata while listing namespace groups"
                );
                return parse_api_error(err).into_response();
            }
        };
        entries.push(NamespaceGroupEntryApiResponse {
            group_id: hex::encode(group_id.to_bytes()),
            name,
        });
    }

    ApiResponse {
        payload: ListNamespaceGroupsApiResponse { data: entries },
    }
    .into_response()
}
