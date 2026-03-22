use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::ManageContextAllowlistRequest;
use calimero_server_primitives::admin::ManageContextAllowlistApiRequest;
use tracing::{error, info};

use super::{parse_context_id, parse_group_id};
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse, Empty};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path((group_id_str, context_id_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    ValidatedJson(req): ValidatedJson<ManageContextAllowlistApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let context_id = match parse_context_id(&context_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(
        group_id=%group_id_str,
        context_id=%context_id_str,
        add_count=req.add.len(),
        remove_count=req.remove.len(),
        "Managing context allowlist"
    );

    let result = state
        .ctx_client
        .manage_context_allowlist(ManageContextAllowlistRequest {
            group_id,
            context_id,
            add: req.add,
            remove: req.remove,
            requester: auth_key.map(|Extension(k)| k.0).or(req.requester),
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, context_id=%context_id_str, "Context allowlist updated");
            ApiResponse { payload: Empty }.into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, context_id=%context_id_str, error=?err, "Failed to manage context allowlist");
            err.into_response()
        }
    }
}
