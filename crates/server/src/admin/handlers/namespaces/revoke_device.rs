use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::{DeviceId, SignedDeviceRevocation};
use calimero_context_client::group::RevokeDeviceRequest;
use calimero_server_primitives::admin::{
    RevocationOutcomeApiEntry, RevokeDeviceApiRequest, RevokeDeviceApiResponse,
    RevokeDeviceApiResponseData,
};
use reqwest::StatusCode;
use tracing::info;

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

/// Decode a hex, borsh-encoded [`SignedDeviceRevocation`].
///
/// The error text names the stage that failed, because the two are different
/// mistakes: bad hex is a transport or copy-paste problem, while hex that is not a
/// proof usually means the wrong blob was pasted.
fn decode_proof(raw: &str) -> Result<SignedDeviceRevocation, String> {
    let bytes = hex::decode(raw.trim()).map_err(|err| format!("proof is not valid hex: {err}"))?;
    borsh::from_slice(&bytes).map_err(|err| {
        format!(
            "proof is valid hex but not a revocation proof: {err}. It should be the \
             output of `merod account revoke-proof`."
        )
    })
}

/// Withdraw a device from an account, terminally.
///
/// An admin may revoke any device; the account holder may revoke its own with a
/// root-signed proof. Only the admin path rotates the scope key, so a
/// self-service revocation stops the device writing at once and leaves it able
/// to read until an admin rotates — reported back rather than hidden.
///
/// The proof may also arrive from the **caller**, minted offline by whoever holds
/// the account root. That is the lost-device path: the root never reaches a node,
/// and the node publishing the revocation needs no authority of its own.
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

    // Decoded here rather than in `validate`, which cannot see the account and so
    // could only confirm the string is hex. A proof that does not deserialize is a
    // caller error worth naming precisely — the alternative is publishing an op
    // whose proof every replica silently declines to honour.
    let proof = match req.proof.as_deref().map(decode_proof).transpose() {
        Ok(proof) => proof,
        Err(message) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message,
            }
            .into_response();
        }
    };

    info!(
        namespace_id = %namespace_id_str,
        device = %req.device_id,
        with_proof = proof.is_some(),
        "revoking a device"
    );

    let result = state
        .ctx_client
        .revoke_device(RevokeDeviceRequest {
            namespace_id,
            device: DeviceId::from(device),
            proof,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(
                namespace_id = %namespace_id_str,
                account = %resp.account,
                device = %resp.device,
                namespaces = resp.revoked_in.len(),
                "device revoked"
            );
            ApiResponse {
                payload: RevokeDeviceApiResponse {
                    data: RevokeDeviceApiResponseData {
                        account_id: hex::encode(resp.account.as_bytes()),
                        device_id: hex::encode(resp.device.as_bytes()),
                        revoked_in: resp
                            .revoked_in
                            .iter()
                            .map(|o| RevocationOutcomeApiEntry {
                                namespace_id: hex::encode(o.namespace_id.to_bytes()),
                                key_rotated: o.key_rotated,
                            })
                            .collect(),
                    },
                },
            }
            .into_response()
        }
        Err(err) => err.into_response(),
    }
}
