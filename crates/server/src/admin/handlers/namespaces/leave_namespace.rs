use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::LeaveNamespaceRequest;
use calimero_server_primitives::admin::{LeaveNamespaceApiResponse, LeaveNamespaceApiResponseData};
use tracing::{error, info};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path(namespace_id_str): Path<String>,
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
        "Leaving namespace (publishing MemberLeft at root, cascading through descendants)"
    );

    let result = state
        .ctx_client
        .leave_namespace(LeaveNamespaceRequest { namespace_id })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(
                namespace_id=%namespace_id_str,
                member=%resp.member_public_key,
                "Successfully left namespace"
            );
            ApiResponse {
                payload: LeaveNamespaceApiResponse {
                    data: LeaveNamespaceApiResponseData {
                        namespace_id: hex::encode(resp.namespace_id.to_bytes()),
                        member_public_key: resp.member_public_key,
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
