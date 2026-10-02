use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::TransferOwnershipRequest;
use calimero_server_primitives::admin::{RootGuardedOpApiResponse, TransferOwnershipApiRequest};
use tracing::info;

use super::parse_group_id;
use crate::admin::handlers::root_proof;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

/// `POST /groups/{group_id}/transfer-ownership`: hand the group to one of its
/// admins. Owner-only, with the owner account's root proof; see
/// [`TransferOwnershipApiRequest`].
pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<TransferOwnershipApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    let new_owner = match root_proof::account("newOwner", &req.new_owner) {
        Ok(account) => account,
        Err(err) => return err.into_response(),
    };
    let root_proof = match root_proof::decode(req.root_proof.as_deref()) {
        Ok(proof) => proof,
        Err(err) => return err.into_response(),
    };

    info!(group_id = %group_id_str, %new_owner, with_proof = root_proof.is_some(), "transferring group ownership");

    match state
        .ctx_client
        .transfer_ownership(TransferOwnershipRequest {
            group_id,
            new_owner,
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
