use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::UpdateGroupSettingsRequest;
use calimero_server_primitives::admin::UpdateGroupSettingsApiRequest;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse, Empty};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<UpdateGroupSettingsApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Updating group settings");

    let result = state
        .ctx_client
        .update_group_settings(UpdateGroupSettingsRequest {
            group_id,
            requester: req.requester,
            upgrade_policy: req.upgrade_policy,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, "Group settings updated successfully");
            ApiResponse { payload: Empty }.into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to update group settings");
            err.into_response()
        }
    }
}
