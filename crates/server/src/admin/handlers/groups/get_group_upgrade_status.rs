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
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

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
