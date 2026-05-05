use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::GetGroupInfoRequest;
use calimero_server_primitives::admin::{GroupInfoApiResponse, GroupInfoApiResponseData};
use tracing::{error, info};

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

    info!(group_id=%group_id_str, "Getting group info");

    let result = state
        .ctx_client
        .get_group_info(GetGroupInfoRequest { group_id })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(info) => {
            info!(group_id=%group_id_str, "Group info retrieved successfully");
            let active_upgrade = info.active_upgrade.as_ref().map(upgrade_info_to_api_data);

            ApiResponse {
                payload: GroupInfoApiResponse {
                    data: GroupInfoApiResponseData {
                        group_id: hex::encode(info.group_id.to_bytes()),
                        app_key: hex::encode(info.app_key.to_bytes()),
                        target_application_id: info.target_application_id,
                        upgrade_policy: info.upgrade_policy,
                        member_count: info.member_count,
                        context_count: info.context_count,
                        active_upgrade,
                        default_capabilities: info.default_capabilities,
                        subgroup_visibility: info.subgroup_visibility,
                        alias: info.alias,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to get group info");
            err.into_response()
        }
    }
}
