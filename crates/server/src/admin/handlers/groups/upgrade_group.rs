use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_context_primitives::group::UpgradeGroupRequest;
use calimero_context_primitives::messages::MigrationParams;
use calimero_server_primitives::admin::{
    UpgradeGroupApiRequest, UpgradeGroupApiResponse, UpgradeGroupApiResponseData,
};
use calimero_store::key::GroupUpgradeStatus;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<UpgradeGroupApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, %req.target_application_id, "Initiating group upgrade");

    let migration = req.migrate_method.map(|method| MigrationParams { method });

    let result = state
        .ctx_client
        .upgrade_group(UpgradeGroupRequest {
            group_id,
            target_application_id: req.target_application_id,
            requester: req.requester,
            migration,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            let (status_str, total, completed, failed) = format_status(&resp.status);
            info!(group_id=%group_id_str, %status_str, "Group upgrade initiated");
            ApiResponse {
                payload: UpgradeGroupApiResponse {
                    data: UpgradeGroupApiResponseData {
                        group_id: hex::encode(resp.group_id.to_bytes()),
                        status: status_str,
                        total,
                        completed,
                        failed,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to initiate group upgrade");
            err.into_response()
        }
    }
}

pub fn format_status(
    status: &GroupUpgradeStatus,
) -> (String, Option<u32>, Option<u32>, Option<u32>) {
    match status {
        GroupUpgradeStatus::InProgress {
            total,
            completed,
            failed,
        } => (
            "in_progress".to_owned(),
            Some(*total),
            Some(*completed),
            Some(*failed),
        ),
        GroupUpgradeStatus::Completed { .. } => ("completed".to_owned(), None, None, None),
        GroupUpgradeStatus::RolledBack { .. } => ("rolled_back".to_owned(), None, None, None),
    }
}
