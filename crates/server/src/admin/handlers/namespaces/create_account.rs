use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::CreateAccountRequest;
use calimero_server_primitives::admin::{
    CreateAccountApiRequest, CreateAccountApiResponse, CreateAccountApiResponseData,
};
use tracing::info;

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

/// Enroll this node's device into a namespace under a fresh account.
///
/// Deliberately takes no caller-supplied identity. The account is rooted at this
/// node's own namespace identity, so there is nothing to choose — and nothing a
/// caller could spoof to enroll a device into somebody else's account.
pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(_req): ValidatedJson<CreateAccountApiRequest>,
) -> impl IntoResponse {
    let namespace_id = match super::super::groups::parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(namespace_id = %namespace_id_str, "enrolling this node's device");

    let result = state
        .ctx_client
        .create_account(CreateAccountRequest { namespace_id })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(
                namespace_id = %namespace_id_str,
                account = %resp.account,
                device = %resp.device,
                "device enrolled"
            );
            ApiResponse {
                payload: CreateAccountApiResponse {
                    data: CreateAccountApiResponseData {
                        account_id: hex::encode(resp.account.as_bytes()),
                        device_id: hex::encode(resp.device.as_bytes()),
                        account_root_key: hex::encode(AsRef::<[u8; 32]>::as_ref(
                            &resp.genesis.root_sign_pk,
                        )),
                        account_nonce: hex::encode(resp.genesis.nonce),
                    },
                },
            }
            .into_response()
        }
        Err(err) => err.into_response(),
    }
}
