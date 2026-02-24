use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::GetGroupUpgradeStatusRequest;
use calimero_server_primitives::admin::{
    GetGroupUpgradeStatusApiResponse, GroupUpgradeStatusApiData,
};
use calimero_store::key::GroupUpgradeStatus;
use tracing::{error, info};

use super::parse_group_id;
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
            let data = upgrade.map(|u| {
                let (status, total, completed, failed, completed_at) = match &u.status {
                    GroupUpgradeStatus::InProgress {
                        total,
                        completed,
                        failed,
                    } => (
                        "in_progress",
                        Some(*total),
                        Some(*completed),
                        Some(*failed),
                        None,
                    ),
                    GroupUpgradeStatus::Completed { completed_at } => {
                        ("completed", None, None, None, Some(*completed_at))
                    }
                };

                GroupUpgradeStatusApiData {
                    from_revision: u.from_revision,
                    to_revision: u.to_revision,
                    initiated_at: u.initiated_at,
                    initiated_by: u.initiated_by,
                    status: status.to_owned(),
                    total,
                    completed,
                    failed,
                    completed_at,
                }
            });

            ApiResponse {
                payload: GetGroupUpgradeStatusApiResponse { data },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to get upgrade status");
            err.into_response()
        }
    }
}
