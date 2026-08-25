use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::{DeviceId, KemPublicKey};
use calimero_context_client::group::{PairDeviceCompleteRequest, PairingScope};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    AccountPairCompleteApiRequest, PairDeviceCompleteApiResponse, PairDeviceCompleteApiResponseData,
};
use reqwest::StatusCode;
use tracing::info;

use crate::admin::handlers::account::{decode32, decode64};
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

/// Certify a device another node minted, link it, and deliver the scope keys.
///
/// The second half of pairing. Run on the node that holds the account — it is
/// the only one with the account root that can sign the certificate.
///
/// `applications` decides which namespaces the link is published into; naming
/// none means every namespace this node takes part in, which is what the fan-out
/// did unconditionally before there was anything to narrow it with.
pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<AccountPairCompleteApiRequest>,
) -> impl IntoResponse {
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

    let mut applications = Vec::with_capacity(req.applications.len());
    for application_id in &req.applications {
        match application_id.parse::<ApplicationId>() {
            Ok(id) => applications.push(id),
            Err(_) => {
                return ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: format!("Invalid application id: {application_id}"),
                }
                .into_response()
            }
        }
    }

    info!(
        applications = applications.len(),
        %device,
        "certifying and linking a paired device"
    );

    let result = state
        .ctx_client
        .pair_device_complete(PairDeviceCompleteRequest {
            scope: PairingScope::Applications(applications),
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
