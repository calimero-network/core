use calimero_governance_store::{MetaRepository, MetadataRepository};
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::GetNamespaceIdentityRequest;
use reqwest::StatusCode;
use tracing::{error, info};

use axum::response::Response;
use calimero_context_config::types::ContextGroupId;
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
    let group_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    // Resolved before any read, so a refusal costs one membership lookup rather
    // than a namespace identity fetch and a metadata load.
    let scope = match list_scope_for(&state.ctx_client, node_owner, account, device) {
        Ok(scope) => scope,
        Err(err) => {
            error!(error=?err, "Failed to resolve the caller's list scope");
            return parse_api_error(err).into_response();
        }
    };
    if let Some(refusal) = refuse_unless_in_scope(&scope, &group_id) {
        return refusal;
    }

    let node_pk = match state
        .ctx_client
        .get_namespace_identity(GetNamespaceIdentityRequest { group_id })
        .await
    {
        Ok(Some(identity)) => identity.public_key,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "No namespace identity found".to_owned(),
            }
            .into_response();
        }
        Err(err) => {
            error!(error=?err, "Failed to get namespace identity");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to get namespace identity".to_owned(),
            }
            .into_response();
        }
    };

    info!(namespace_id=%namespace_id_str, "Getting namespace summary");

    let meta = match MetaRepository::new(&state.store).load(&group_id) {
        Ok(Some(meta)) => meta,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Namespace not found".to_owned(),
            }
            .into_response();
        }
        Err(err) => {
            error!(?err, "Failed to load namespace metadata");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to load namespace metadata".to_owned(),
            }
            .into_response();
        }
    };

    match MetadataRepository::new(&state.store).build_namespace_summary(&group_id, &meta, &node_pk)
    {
        Ok(Some(ns)) => {
            let app_version =
                super::namespace_app_version(&state.node_client, ns.bytecode_id.to_bytes()).await;
            ApiResponse {
                payload: calimero_server_primitives::admin::GetNamespaceApiResponse {
                    data: calimero_server_primitives::admin::NamespaceApiResponse {
                        namespace_id: hex::encode(ns.namespace_id.to_bytes()),
                        bytecode_id: hex::encode(ns.bytecode_id.to_bytes()),
                        target_application_id: ns.target_application_id.to_string(),
                        created_at: ns.created_at,
                        name: ns.name,
                        member_count: ns.member_count,
                        context_count: ns.context_count,
                        subgroup_count: ns.subgroup_count,
                        app_version,
                    },
                },
            }
            .into_response()
        }
        Ok(None) => ApiError {
            status_code: StatusCode::NOT_FOUND,
            message: "Namespace not found".to_owned(),
        }
        .into_response(),
        Err(err) => {
            error!(?err, "Failed to build namespace summary");
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to build namespace summary".to_owned(),
            }
            .into_response()
        }
    }
}
