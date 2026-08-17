use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::UpdateMemberRoleRequest;
use calimero_server_primitives::admin::UpdateMemberRoleApiRequest;
use tracing::{error, info};

use super::{parse_account, parse_group_id};
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::parse_api_error;
use crate::admin::service::ApiResponse;
use crate::AdminState;
use calimero_server_primitives::admin::UpdateMemberRoleApiResponse;

pub async fn handler(
    Path((group_id_str, account_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<UpdateMemberRoleApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let account = match parse_account(&account_str) {
        Ok(account) => account,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, identity=%account_str, "Updating member role");

    let result = state
        .ctx_client
        .update_member_role(UpdateMemberRoleRequest {
            group_id,
            identity: account,
            new_role: req.role,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, identity=%account_str, "Member role updated successfully");
            ApiResponse {
                payload: UpdateMemberRoleApiResponse {},
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, identity=%account_str, error=?err, "Failed to update member role");
            err.into_response()
        }
    }
}
