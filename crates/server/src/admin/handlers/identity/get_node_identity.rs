use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{NodeIdentityApiResponse, NodeIdentityApiResponseData};
use reqwest::StatusCode;
use tracing::error;

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

/// Who this node is: the account it writes as, the device it is, and the key it
/// signs with.
///
/// Takes no namespace, and that is the whole point of it. Each of the three is
/// node-level, so a namespace in the path could only ever be decoration — the
/// endpoints it replaces took one and returned the same answer regardless, which
/// read as though the answer varied.
pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    let store = state.ctx_client.datastore();

    let devices = calimero_governance_store::NodeDeviceRepository::new(store);

    // The DEVICE ROW first, and the account root only as a fallback.
    //
    // The row names the account this node speaks for, and it is the only place
    // that account is written down: a PAIRED node adopted an account rooted at
    // another node's key, so it holds no root of its own — pairing mints a device
    // (`ensure_enrolled_into`) without ever minting a root. Starting from the root
    // therefore answered 404 for exactly the node whose identity a caller most
    // needs, and would have answered with the WRONG account had the node happened
    // to hold a root as well: a locally derived id no row in the group is keyed by.
    //
    // A failed READ is not a missing row, and reporting them alike would tell an
    // operator "not enrolled" when the truth is "could not look".
    let held = match devices.get() {
        Ok(held) => held,
        Err(err) => {
            error!(error = ?err, "Failed to read this node's device row");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to read this node's device".to_owned(),
            }
            .into_response();
        }
    };

    let (account, account_root_pk, device, agreement_key) = match held {
        Some(held) => (
            held.account,
            held.genesis.root_sign_pk,
            Some(hex::encode(held.device().as_bytes())),
            // Through the crate's own accessor, not by reaching into the secret:
            // `kem_public_key` is what certificates already publish, so the value
            // reported here cannot drift from the one that gets certified.
            Some(hex::encode(held.kem_public_key().as_bytes())),
        ),
        // No device row: this node speaks only for itself, so its own root answers
        // — and a node with neither has taken part in nothing at all.
        None => {
            let Some(root) = (match devices.account_root() {
                Ok(root) => root,
                Err(err) => {
                    error!(error = ?err, "Failed to read this node's account root");
                    return ApiError {
                        status_code: StatusCode::INTERNAL_SERVER_ERROR,
                        message: "Failed to read this node's account root".to_owned(),
                    }
                    .into_response();
                }
            }) else {
                return ApiError {
                    status_code: StatusCode::NOT_FOUND,
                    message: "this node holds neither a device nor an account root yet; \
                              both are minted the first time it takes part in a namespace"
                        .to_owned(),
                }
                .into_response();
            };
            // No device row means no agreement key: it is the device's, and this
            // branch is reached precisely because there is no device.
            (root.account(), root.genesis().root_sign_pk, None, None)
        }
    };

    // The key this node SIGNS with, which is the device's — not the account
    // root's. The root signs certificates and handoffs and never an op, so
    // reporting it here would name a key no signature on the wire verifies
    // against.
    let signing_key =
        match calimero_governance_store::NamespaceRepository::new(store).node_identity() {
            Ok(Some(record)) => record.public_key.to_string(),
            Ok(None) => {
                return ApiError {
                    status_code: StatusCode::NOT_FOUND,
                    message: "this node holds an account root but no signing identity yet; \
                          one is provisioned when it first joins a namespace"
                        .to_owned(),
                }
                .into_response();
            }
            Err(err) => {
                error!(error = ?err, "Failed to read this node's signing identity");
                return ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "Failed to read this node's signing identity".to_owned(),
                }
                .into_response();
            }
        };

    ApiResponse {
        payload: NodeIdentityApiResponse {
            data: NodeIdentityApiResponseData {
                account_id: hex::encode(account.as_bytes()),
                device_id: device,
                public_key: signing_key,
                // The root of the account this node SPEAKS FOR, which for a paired
                // node belongs to another machine. Public by construction — it is
                // hashed into the account id and travels in every genesis — and it
                // is what a further device needs in order to pair into the same
                // account.
                account_root_public_key: hex::encode(AsRef::<[u8; 32]>::as_ref(&account_root_pk)),
                // The third input `sign-cert` needs. An operator holding the
                // offline root can now read all three from one call and certify
                // this node's device without the node ever touching the root.
                device_agreement_key: agreement_key,
            },
        },
    }
    .into_response()
}
