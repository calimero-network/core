use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::AccountGenesis;
use calimero_context_client::group::PairDeviceInitRequest;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    AccountPairInitApiRequest, PairDeviceInitApiResponse, PairDeviceInitApiResponseData,
};
use tracing::info;

use crate::admin::handlers::account::decode32;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

/// Mint a device on this node for an account that already exists elsewhere.
///
/// The first half of pairing. It publishes nothing and needs no scope key — it
/// produces the `DeviceId` and KEM key that the account holder's
/// `pair-complete` will certify, the device's signature over them, and the code
/// to read out to the account holder.
///
/// One device covers every namespace named, so there is one of each to hand
/// over however many the caller listed. The list is the caller's to supply: this
/// node is a member of nothing and cannot discover which namespaces the account
/// speaks in.
pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<AccountPairInitApiRequest>,
) -> impl IntoResponse {
    let root_key = match decode32(&req.account_root_public_key, "accountRootPublicKey") {
        Ok(bytes) => bytes,
        Err(err) => return err.into_response(),
    };
    let genesis = AccountGenesis::new(PublicKey::from(root_key));

    let mut namespaces = Vec::with_capacity(req.namespaces.len());
    for namespace_id in &req.namespaces {
        match decode32(namespace_id, "namespaces[]") {
            Ok(bytes) => namespaces.push(bytes.into()),
            Err(err) => return err.into_response(),
        }
    }

    info!(
        namespaces = namespaces.len(),
        account = %genesis.account_id(),
        "minting a device for an existing account"
    );

    let result = state
        .ctx_client
        .pair_device_init(PairDeviceInitRequest {
            namespaces,
            genesis,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(
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
