use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::SetMemberAliasRequest;
use calimero_server_primitives::admin::SetMemberAliasApiRequest;
use tracing::{error, info};

use super::{parse_group_id, parse_identity};
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse, Empty};
use crate::AdminState;

pub async fn handler(
    Path((group_id_str, identity_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<SetMemberAliasApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let member = match parse_identity(&identity_str) {
        Ok(pk) => pk,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, identity=%identity_str, alias=%req.alias, "Setting member alias");

    let result = state
        .ctx_client
        .set_member_alias(SetMemberAliasRequest {
            group_id,
            member,
            alias: req.alias,
            requester: req.requester,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, identity=%identity_str, "Member alias set");
            ApiResponse { payload: Empty }.into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, identity=%identity_str, error=?err, "Failed to set member alias");
            err.into_response()
        }
    }
}
