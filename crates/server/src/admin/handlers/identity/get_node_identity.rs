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

    let Some(root) =
        (match calimero_governance_store::NodeDeviceRepository::new(store).account_root() {
            Ok(root) => root,
            Err(err) => {
                error!(error = ?err, "Failed to read this node's account root");
                return ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "Failed to read this node's account root".to_owned(),
                }
                .into_response();
            }
        })
    else {
        // No root means the node has never taken part in anything: an account is
        // the content address of a root key, so without one there is no account
        // to report rather than an empty one.
        return ApiError {
            status_code: StatusCode::NOT_FOUND,
            message: "this node holds no account root yet; it is minted the first \
                      time the node enrolls in a namespace"
                .to_owned(),
        }
        .into_response();
    };

    // A missing device is a real answer — the root exists but nothing has enrolled
    // yet — while a failed READ is not, and reporting them the same way would tell
    // an operator "not enrolled" when the truth is "could not look".
    let device = match calimero_governance_store::NodeDeviceRepository::new(store).get() {
        Ok(enrolled) => enrolled.map(|enrolled| hex::encode(enrolled.device().as_bytes())),
        Err(err) => {
            error!(error = ?err, "Failed to read this node's device row");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to read this node's device".to_owned(),
            }
            .into_response();
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
                account_id: hex::encode(root.account().as_bytes()),
                device_id: device,
                public_key: signing_key,
                account_root_public_key: hex::encode(AsRef::<[u8; 32]>::as_ref(
                    &root.genesis().root_sign_pk,
                )),
            },
        },
    }
    .into_response()
}
