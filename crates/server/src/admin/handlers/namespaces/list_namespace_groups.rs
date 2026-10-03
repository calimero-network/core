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
use crate::admin::handlers::groups::list_subgroups::visible_children;
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

    let scope = match list_scope_for(&state.ctx_client, node_owner, account.clone(), device) {
        Ok(scope) => scope,
        Err(err) => {
            error!(error=?err, "Failed to resolve the caller's list scope");
            return parse_api_error(err).into_response();
        }
    };
    if let Some(refusal) = refuse_unless_in_scope(&scope, &namespace_id) {
        return refusal;
    }

    let entries = match visible_children(&state, &namespace_id, account.map(|Extension(a)| a)) {
        Ok(children) => children
            .into_iter()
            .map(|(group_id, name)| NamespaceGroupEntryApiResponse {
                group_id: hex::encode(group_id.to_bytes()),
                name,
            })
            .collect(),
        Err(err) => return parse_api_error(err).into_response(),
    };

    ApiResponse {
        payload: ListNamespaceGroupsApiResponse { data: entries },
    }
    .into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::{Extension, Router};
    use calimero_context_client::client::ContextClient;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::MembershipRepository;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PublicKey;
    use calimero_server_primitives::admin::ListNamespaceGroupsApiResponse;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use tower::ServiceExt;

    use super::handler;
    use crate::auth::AuthenticatedAccount;
    use crate::{AdminState, NodeReadiness};

    /// Whether the namespace listing names its Restricted subgroup to an account
    /// holding `role` in the namespace, and a row in the subgroup if `in_subgroup`.
    async fn lists_the_restricted_subgroup(role: GroupMemberRole, in_subgroup: bool) -> bool {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (namespace, subgroup, account) =
            crate::test_support::seed_namespace_with_restricted_subgroup(
                &store,
                PublicKey::from([0x5C; 32]),
                role,
            );
        if in_subgroup {
            MembershipRepository::new(&store)
                .add_member(
                    &ContextGroupId::from(*subgroup.as_bytes()),
                    &account,
                    GroupMemberRole::Member,
                )
                .expect("seat the account in the subgroup");
        }
        let (event_sender, _rx) = tokio::sync::broadcast::channel(16);
        let (node_client, _blob_dir) = crate::test_support::test_node_client(
            &store,
            crate::test_support::stub_node_manager(vec![]),
            event_sender,
        )
        .await;
        let ctx_client =
            ContextClient::new(store.clone(), node_client.clone(), LazyRecipient::new());
        let state = Arc::new(AdminState::new(
            store,
            ctx_client,
            node_client,
            Arc::new(NodeReadiness::new()),
            [0; 32],
            #[cfg(feature = "mock-attestation")]
            false,
        ));
        let app = Router::new()
            .route("/namespaces/{namespace_id}/groups", get(handler))
            .layer(Extension(state))
            .layer(Extension(AuthenticatedAccount(account)));

        let uri = format!("/namespaces/{}/groups", hex::encode(namespace.as_bytes()));
        let response = app
            .oneshot(Request::get(uri).body(Body::empty()).expect("a request"))
            .await
            .expect("the listing route answers");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read the response");
        let listed: ListNamespaceGroupsApiResponse =
            serde_json::from_slice(&body).expect("a listing");
        let subgroup = hex::encode(subgroup.as_bytes());
        listed.data.iter().any(|entry| entry.group_id == subgroup)
    }

    #[actix::test]
    async fn a_restricted_subgroup_is_listed_only_to_the_namespace_admin_and_its_members() {
        assert!(
            lists_the_restricted_subgroup(GroupMemberRole::Admin, false).await,
            "precondition: the namespace admin sees it"
        );
        assert!(lists_the_restricted_subgroup(GroupMemberRole::Member, true).await);
        assert!(!lists_the_restricted_subgroup(GroupMemberRole::Member, false).await);
    }
}
