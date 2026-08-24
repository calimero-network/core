use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::{DeviceId, KemPublicKey};
use calimero_context_client::group::PairDeviceCompleteRequest;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    PairDeviceCompleteApiRequest, PairDeviceCompleteApiResponse, PairDeviceCompleteApiResponseData,
};
use reqwest::StatusCode;
use tracing::info;

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

/// Decode a 64-hex-char field into 32 bytes. Lengths are already validated; the
/// decode is still fallible because validation and parsing are separate layers.
fn decode32(value: &str, field: &str) -> Result<[u8; 32], ApiError> {
    hex::decode(value)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("{field} must be 64 hex chars (32 bytes)"),
        })
}

/// Same, for the 64-byte pairing statement.
fn decode64(value: &str, field: &str) -> Result<[u8; 64], ApiError> {
    hex::decode(value)
        .ok()
        .and_then(|b| <[u8; 64]>::try_from(b).ok())
        .ok_or_else(|| ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("{field} must be 128 hex chars (64 bytes)"),
        })
}

/// Certify a device another node minted, link it, and deliver the scope key.
///
/// The second half of pairing. Run on the node that holds the account — it is
/// the only one with the account root that can sign the certificate.
pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<PairDeviceCompleteApiRequest>,
) -> impl IntoResponse {
    let namespace_id = match super::super::groups::parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let device = match decode32(&req.device_id, "deviceId") {
        Ok(bytes) => DeviceId::from(bytes),
        Err(err) => return err.into_response(),
    };
    let kem_pk = match decode32(&req.kem_public_key, "kemPublicKey") {
        Ok(bytes) => KemPublicKey::from(bytes),
        Err(err) => return err.into_response(),
    };
    let sign_pk = match decode32(&req.sign_public_key, "signPublicKey") {
        Ok(bytes) => PublicKey::from(bytes),
        Err(err) => return err.into_response(),
    };
    let statement = match decode64(&req.statement, "statement") {
        Ok(bytes) => bytes,
        Err(err) => return err.into_response(),
    };

    info!(
        namespace_id = %namespace_id_str,
        %device,
        "certifying and linking a paired device"
    );

    let result = state
        .ctx_client
        .pair_device_complete(PairDeviceCompleteRequest {
            namespace_id,
            device,
            kem_pk,
            sign_pk,
            statement,
            confirmation_code: req.confirmation_code,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(
                namespace_id = %namespace_id_str,
                account = %resp.account,
                device = %resp.device,
                key_delivered = resp.key_delivered,
                confirmation_code = %resp.confirmation_code,
                "paired device linked"
            );
            ApiResponse {
                payload: PairDeviceCompleteApiResponse {
                    data: PairDeviceCompleteApiResponseData {
                        account_id: hex::encode(resp.account.as_bytes()),
                        device_id: hex::encode(resp.device.as_bytes()),
                        key_delivered: resp.key_delivered,
                        confirmation_code: resp.confirmation_code,
                        // Borsh, hex-encoded: the canonical form of a signed
                        // credential is its encoding, and a JSON restatement
                        // would be a second spelling that could disagree with
                        // the bytes the root actually signed.
                        credential: borsh::to_vec(&*resp.credential)
                            .map(hex::encode)
                            .unwrap_or_default(),
                    },
                },
            }
            .into_response()
        }
        Err(err) => err.into_response(),
    }
}
