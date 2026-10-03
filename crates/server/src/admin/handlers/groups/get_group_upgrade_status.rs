use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::GetGroupUpgradeStatusRequest;
use calimero_server_primitives::admin::GetGroupUpgradeStatusApiResponse;
use tracing::{debug, error, info};

use super::{parse_group_id, upgrade_info_to_api_data};
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    node_owner: Option<Extension<crate::auth::AuthenticatedNodeOwner>>,
    account: Option<Extension<crate::auth::AuthenticatedAccount>>,
    device: Option<Extension<crate::auth::AuthenticatedDevice>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    // Before any read: a delegated session reaches this route on the narrow
    // `group:list-own`, so it must be confined to its own groups here.
    if let Some(refusal) = crate::admin::caller_scope::refuse_group_outside_caller_scope(
        &state.ctx_client,
        node_owner,
        account,
        device,
        &group_id,
    ) {
        return refusal;
    }

    info!(group_id=%group_id_str, "Getting group upgrade status");

    let result = state
        .ctx_client
        .get_group_upgrade_status(GetGroupUpgradeStatusRequest { group_id })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(upgrade) => {
            let data = upgrade.as_ref().map(upgrade_info_to_api_data);

            ApiResponse {
                payload: GetGroupUpgradeStatusApiResponse { data },
            }
            .into_response()
        }
        Err(err) => {
            // A 4xx is the caller naming something absent or not theirs, not a
            // fault of this node. On a fleet node these journals ship to a
            // central store, where an `ERROR` per poll buries real faults.
            if err.is_client_fault() {
                debug!(group_id=%group_id_str, error=?err, "Failed to get upgrade status");
            } else {
                error!(group_id=%group_id_str, error=?err, "Failed to get upgrade status");
            }
            err.into_response()
        }
    }
}
