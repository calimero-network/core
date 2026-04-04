use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::DeleteGroupRequest;
use calimero_server_primitives::admin::{
    DeleteNamespaceApiRequest, DeleteNamespaceApiResponse, DeleteNamespaceApiResponseData,
};
use tracing::{error, info};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    ValidatedJson(req): ValidatedJson<DeleteNamespaceApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(namespace_id=%namespace_id_str, "Deleting namespace");

    let requester = auth_key.map(|Extension(k)| k.0).or(req.requester);

    let result = state
        .ctx_client
        .delete_group(DeleteGroupRequest {
            group_id,
            requester,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(response) => {
            info!(namespace_id=%namespace_id_str, deleted=%response.deleted, "Namespace deletion completed");
            ApiResponse {
                payload: DeleteNamespaceApiResponse {
                    data: DeleteNamespaceApiResponseData {
                        is_deleted: response.deleted,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(namespace_id=%namespace_id_str, error=?err, "Failed to delete namespace");
            err.into_response()
        }
    }
}
