pub mod add_group_members;
pub mod create_group;
pub mod create_group_invitation;
pub mod delete_group;
pub mod detach_context_from_group;
pub mod get_group_info;
pub mod get_group_upgrade_status;
pub mod join_group;
pub mod join_group_context;
pub mod list_all_groups;
pub mod list_group_contexts;
pub mod list_group_members;
pub mod register_signing_key;
pub mod remove_group_members;
pub mod retry_group_upgrade;
pub mod sync_group;
pub mod update_group_settings;
pub mod update_member_role;
pub mod upgrade_group;

use calimero_context_config::types::ContextGroupId;
use calimero_context_primitives::group::{GroupUpgradeInfo, GroupUpgradeStatus};
use calimero_server_primitives::admin::GroupUpgradeStatusApiData;
use reqwest::StatusCode;

use crate::admin::service::ApiError;

fn upgrade_info_to_api_data(info: &GroupUpgradeInfo) -> GroupUpgradeStatusApiData {
    let (status, total, completed, failed, completed_at) = match &info.status {
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
        from_version: info.from_version.clone(),
        to_version: info.to_version.clone(),
        initiated_at: info.initiated_at,
        initiated_by: info.initiated_by,
        status: status.to_owned(),
        total,
        completed,
        failed,
        completed_at,
    }
}

fn decode_signing_key(hex_str: &str) -> Result<[u8; 32], ApiError> {
    let bytes = hex::decode(hex_str).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid requester_secret: expected hex-encoded 32 bytes".into(),
    })?;
    bytes.try_into().map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid requester_secret: must be exactly 32 bytes".into(),
    })
}

fn parse_group_id(s: &str) -> Result<ContextGroupId, ApiError> {
    let bytes = hex::decode(s).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid group id format: expected hex-encoded 32 bytes".into(),
    })?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid group id: must be exactly 32 bytes".into(),
    })?;
    Ok(ContextGroupId::from(arr))
}
