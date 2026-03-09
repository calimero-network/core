use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::GetMemberCapabilitiesRequest;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    GetMemberCapabilitiesApiData, GetMemberCapabilitiesApiResponse,
};
use reqwest::StatusCode;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path((group_id_str, identity_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let member = match parse_identity(&identity_str) {
        Ok(pk) => pk,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, identity=%identity_str, "Getting member capabilities");

    let result = state
        .ctx_client
        .get_member_capabilities(GetMemberCapabilitiesRequest { group_id, member })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(response) => {
            info!(group_id=%group_id_str, identity=%identity_str, "Got member capabilities");
            ApiResponse {
                payload: GetMemberCapabilitiesApiResponse {
                    data: GetMemberCapabilitiesApiData {
                        capabilities: response.capabilities,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, identity=%identity_str, error=?err, "Failed to get member capabilities");
            err.into_response()
        }
    }
}

fn parse_identity(s: &str) -> Result<PublicKey, ApiError> {
    let bytes = hex::decode(s).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid identity format: expected hex-encoded 32 bytes".into(),
    })?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid identity: must be exactly 32 bytes".into(),
    })?;
    Ok(PublicKey::from(arr))
}
