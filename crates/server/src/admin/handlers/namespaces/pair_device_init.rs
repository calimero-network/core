use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::AccountGenesis;
use calimero_context_client::group::PairDeviceInitRequest;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    PairDeviceInitApiRequest, PairDeviceInitApiResponse, PairDeviceInitApiResponseData,
};
use reqwest::StatusCode;
use tracing::info;

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

/// Mint a device on this node for an account that already exists elsewhere.
///
/// The first half of pairing. It publishes nothing and needs no scope key — it
/// produces the `DeviceId` and KEM key that the account holder's
/// `pair-complete` will certify, the device's signature over them, and the code
/// to read out to the account holder.
pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<PairDeviceInitApiRequest>,
) -> impl IntoResponse {
    let namespace_id = match super::super::groups::parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    // Lengths are already validated; the decode still has to be handled because
    // validation and parsing are separate layers here.
    let root_key: [u8; 32] = match hex::decode(&req.account_root_key)
        .ok()
        .and_then(|b| b.try_into().ok())
    {
        Some(bytes) => bytes,
        None => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "accountRootKey must be 64 hex chars (32 bytes)".to_owned(),
            }
            .into_response();
        }
    };
    let genesis = AccountGenesis::new(PublicKey::from(root_key));

    info!(
        namespace_id = %namespace_id_str,
        account = %genesis.account_id(),
        "minting a device for an existing account"
    );

    let result = state
        .ctx_client
        .pair_device_init(PairDeviceInitRequest {
            namespace_id,
            genesis,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(
                namespace_id = %namespace_id_str,
                account = %resp.account,
                device = %resp.device,
                "device minted; awaiting its certificate"
            );
            ApiResponse {
                payload: PairDeviceInitApiResponse {
                    data: PairDeviceInitApiResponseData {
                        account_id: hex::encode(resp.account.as_bytes()),
                        device_id: hex::encode(resp.device.as_bytes()),
                        kem_public_key: hex::encode(resp.kem_pk.as_bytes()),
                        sign_public_key: hex::encode(AsRef::<[u8; 32]>::as_ref(&resp.sign_pk)),
                        statement: hex::encode(resp.statement),
                        confirmation_code: resp.confirmation_code,
                    },
                },
            }
            .into_response()
        }
        Err(err) => err.into_response(),
    }
}
