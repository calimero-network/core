pub mod add_group_members;
pub mod create_group;
pub mod create_group_invitation;
pub mod delete_group;
pub mod detach_context_from_group;
pub mod get_context_allowlist;
pub mod get_context_visibility;
pub mod get_group_info;
pub mod get_group_upgrade_status;
pub mod get_member_capabilities;
pub mod join_group;
pub mod join_group_context;
pub mod list_all_groups;
pub mod list_group_contexts;
pub mod list_group_members;
pub mod manage_context_allowlist;
pub mod register_signing_key;
pub mod remove_group_members;
pub mod retry_group_upgrade;
pub mod set_context_visibility;
pub mod set_default_capabilities;
pub mod set_default_visibility;
pub mod set_member_alias;
pub mod set_member_capabilities;
pub mod sync_group;
pub mod update_group_settings;
pub mod update_member_role;
pub mod upgrade_group;

use calimero_context_config::types::ContextGroupId;
use calimero_context_primitives::group::{GroupUpgradeInfo, GroupUpgradeStatus};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
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
            ("completed", None, None, None, *completed_at)
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

fn parse_context_id(s: &str) -> Result<ContextId, ApiError> {
    if let Ok(context_id) = s.parse::<ContextId>() {
        return Ok(context_id);
    }

    let bytes = hex::decode(s).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid context id format: expected base58 or hex-encoded 32 bytes".into(),
    })?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid context id: must be exactly 32 bytes".into(),
    })?;
    Ok(ContextId::from(arr))
}

fn parse_identity(s: &str) -> Result<PublicKey, ApiError> {
    if let Ok(identity) = s.parse::<PublicKey>() {
        return Ok(identity);
    }

    let bytes = hex::decode(s).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid identity format: expected public key or hex-encoded 32 bytes".into(),
    })?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid identity: must be exactly 32 bytes".into(),
    })?;
    Ok(PublicKey::from(arr))
}

#[cfg(test)]
mod tests {
    use calimero_primitives::identity::PublicKey;

    use super::{parse_context_id, parse_identity};

    #[test]
    fn parse_context_id_accepts_base58_context_ids() {
        let context_id = parse_context_id("11111111111111111111111111111111");

        assert!(context_id.is_ok());
    }

    #[test]
    fn parse_context_id_keeps_accepting_hex_context_ids() {
        let context_id =
            parse_context_id("0000000000000000000000000000000000000000000000000000000000000000");

        assert!(context_id.is_ok());
    }

    #[test]
    fn parse_identity_accepts_public_key_strings() {
        let identity = PublicKey::from([0; 32]).to_string();

        let parsed_identity = parse_identity(&identity);

        assert!(parsed_identity.is_ok());
    }

    #[test]
    fn parse_identity_keeps_accepting_hex_identities() {
        let identity =
            parse_identity("0000000000000000000000000000000000000000000000000000000000000000");

        assert!(identity.is_ok());
    }
}
