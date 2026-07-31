use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::DeviceId;
use calimero_context_client::group::RevokeDeviceRequest;
use calimero_server_primitives::admin::{
    RevokeDeviceApiRequest, RevokeDeviceApiResponse, RevokeDeviceApiResponseData,
};
use reqwest::StatusCode;
use tracing::info;

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

/// Withdraw a device from an account, terminally.
///
/// An admin may revoke any device; the account holder may revoke its own with a
/// root-signed proof. Only the admin path rotates the scope key, so a
/// self-service revocation stops the device writing at once and leaves it able
/// to read until an admin rotates — reported back rather than hidden.
pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<RevokeDeviceApiRequest>,
) -> impl IntoResponse {
    let namespace_id = match super::super::groups::parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let device: [u8; 32] = match hex::decode(&req.device_id)
        .ok()
        .and_then(|b| b.try_into().ok())
    {
        Some(bytes) => bytes,
        None => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "deviceId must be 64 hex chars (32 bytes)".to_owned(),
            }
            .into_response();
        }
    };

    info!(namespace_id = %namespace_id_str, device = %req.device_id, "revoking a device");

    let result = state
        .ctx_client
        .revoke_device(RevokeDeviceRequest {
            namespace_id,
            device: DeviceId::from(device),
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(
                namespace_id = %namespace_id_str,
                account = %resp.account,
                device = %resp.device,
                key_rotated = resp.key_rotated,
                "device revoked"
            );
            ApiResponse {
                payload: RevokeDeviceApiResponse {
                    data: RevokeDeviceApiResponseData {
                        account_id: hex::encode(resp.account.as_bytes()),
                        device_id: hex::encode(resp.device.as_bytes()),
                        key_rotated: resp.key_rotated,
                    },
                },
            }
            .into_response()
        }
        Err(err) => err.into_response(),
    }
}
