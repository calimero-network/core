pub mod add_group_members;
pub mod create_group;
pub mod delete_group;
pub mod get_group_info;
pub mod list_group_contexts;
pub mod list_group_members;
pub mod remove_group_members;

use calimero_context_config::types::ContextGroupId;
use reqwest::StatusCode;

use crate::admin::service::ApiError;

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
