use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::ChangeNamespaceAdminRequest;
use calimero_server_primitives::admin::{ChangeNamespaceAdminApiRequest, RootGuardedOpApiResponse};
use tracing::info;

use crate::admin::handlers::root_proof;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

/// `POST /namespaces/{namespace_id}/admin`: repoint the namespace's admin pin.
/// Owner-only, with the owner account's root proof; see
/// [`ChangeNamespaceAdminApiRequest`].
pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<ChangeNamespaceAdminApiRequest>,
) -> impl IntoResponse {
    let namespace_id = match super::super::groups::parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    let new_admin = match root_proof::account("newAdmin", &req.new_admin) {
        Ok(account) => account,
        Err(err) => return err.into_response(),
    };
    let root_proof = match root_proof::decode(req.root_proof.as_deref()) {
        Ok(proof) => proof,
        Err(err) => return err.into_response(),
    };

    info!(namespace_id = %namespace_id_str, %new_admin, with_proof = root_proof.is_some(), "changing namespace admin");

    match state
        .ctx_client
        .change_namespace_admin(ChangeNamespaceAdminRequest {
            namespace_id,
            new_admin,
            root_proof,
        })
        .await
        .map_err(parse_api_error)
    {
        Ok(()) => ApiResponse {
            payload: RootGuardedOpApiResponse {},
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
