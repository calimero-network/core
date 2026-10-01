use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use borsh::BorshDeserialize;
use calimero_account::{AccountProof, DeviceCert, DeviceScope};
use calimero_context_client::group::LinkAccountDeviceRequest;
use calimero_server_primitives::admin::{
    LinkAccountDeviceApiRequest, LinkAccountDeviceApiResponse, LinkAccountDeviceApiResponseData,
};
use reqwest::StatusCode;
use tracing::debug;

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

/// Decode a hex, borsh-encoded proof, naming the field and the stage that failed.
fn decode<T: BorshDeserialize>(raw: &str, field: &str, what: &str) -> Result<T, ApiError> {
    let bad = |message: String| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message,
    };
    let bytes = hex::decode(raw.trim()).map_err(|err| bad(format!("{field} is not hex: {err}")))?;
    borsh::from_slice(&bytes).map_err(|err| bad(format!("{field} is not {what}: {err}")))
}

/// Carry a device link for an account this node does not hold.
///
/// The relay half of a nodeless account minting invitations: the account was
/// added by account, so the namespace binds none of its keys, and peers refuse
/// whatever its device signs. Once this lands, the device's key resolves to the
/// account everywhere and the account's own grants decide.
pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<LinkAccountDeviceApiRequest>,
) -> impl IntoResponse {
    let namespace_id = match super::super::groups::parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    let credential: AccountProof<DeviceCert> =
        match decode(&req.credential, "credential", "a device credential") {
            Ok(proof) => proof,
            Err(err) => return err.into_response(),
        };
    let scope: AccountProof<DeviceScope> = match decode(&req.scope, "scope", "a device scope") {
        Ok(proof) => proof,
        Err(err) => return err.into_response(),
    };

    debug!(
        namespace_id = %namespace_id_str,
        account = %credential.statement.account,
        device = %credential.statement.device,
        "carrying a device link for an account this node does not hold"
    );

    match state
        .ctx_client
        .link_account_device(LinkAccountDeviceRequest {
            namespace_id,
            credential,
            scope,
        })
        .await
        .map_err(parse_api_error)
    {
        Ok(resp) => ApiResponse {
            payload: LinkAccountDeviceApiResponse {
                data: LinkAccountDeviceApiResponseData {
                    account_id: hex::encode(resp.account.as_bytes()),
                    device_id: hex::encode(resp.device.as_bytes()),
                    already_bound: resp.already_bound,
                },
            },
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
