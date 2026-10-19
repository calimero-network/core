use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::LeaveNamespaceRequest;
use calimero_server_primitives::admin::{
    LeaveNamespaceApiResponse, LeaveNamespaceApiResponseData, LeaveNamespaceQuery,
};
use tracing::{debug, error, info};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Query(LeaveNamespaceQuery { purge }): Query<LeaveNamespaceQuery>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let auth_caller = auth_key.map(|Extension(k)| k.0);

    info!(
        namespace_id=%namespace_id_str,
        ?auth_caller,
        purge,
        "Leaving namespace (publishing MemberLeft at root, cascading through descendants)"
    );

    let result = state
        .ctx_client
        .leave_namespace(LeaveNamespaceRequest {
            namespace_id,
            purge,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            debug!(
                namespace_id=%namespace_id_str,
                member=%resp.member_public_key,
                "Successfully left namespace"
            );
            ApiResponse {
                payload: LeaveNamespaceApiResponse {
                    data: LeaveNamespaceApiResponseData {
                        namespace_id: hex::encode(resp.namespace_id.to_bytes()),
                        member_public_key: resp.member_public_key,
                        purged: resp.purged,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(namespace_id=%namespace_id_str, error=?err, "Failed to leave namespace");
            err.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::extract::Query;
    use axum::http::Uri;
    use calimero_server_primitives::admin::LeaveNamespaceQuery;

    fn purge(uri: &str) -> Option<bool> {
        Query::<LeaveNamespaceQuery>::try_from_uri(&uri.parse::<Uri>().expect("a uri"))
            .ok()
            .map(|Query(query)| query.purge)
    }

    #[test]
    fn only_an_explicit_purge_purges_and_a_malformed_one_is_refused() {
        assert_eq!(purge("/namespaces/x/leave"), Some(false));
        assert_eq!(purge("/namespaces/x/leave?purge=false"), Some(false));
        assert_eq!(purge("/namespaces/x/leave?purge=true"), Some(true));
        assert_eq!(purge("/namespaces/x/leave?purge=yes"), None);
        assert_eq!(purge("/namespaces/x/leave?purgee=true"), None);
    }
}
