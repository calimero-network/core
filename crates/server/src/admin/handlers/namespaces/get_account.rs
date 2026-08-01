use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{
    NamespaceAccountApiResponse, NamespaceAccountApiResponseData,
};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

/// Report which account this node speaks for in a namespace, and the device it
/// holds there.
///
/// Reads rather than mints: the account id is *derived* from this node's root and
/// the namespace, so it exists whether or not `account create` has ever run. What
/// may be absent is the device — a node with no enrolled device has an account
/// nobody has heard of yet, which is a meaningful distinction for an operator
/// deciding whether pairing is needed.
///
/// A plain store read, so it goes straight to the datastore rather than through
/// the context actor's mailbox: nothing here mutates, and queueing behind
/// in-flight executions would make an operator's "who am I" wait on a WASM run.
pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(namespace_id = %namespace_id_str, "Resolving this node's account");

    let store = state.ctx_client.datastore();

    // Resolve the namespace ONCE and use it for both reads. An account is scoped to
    // the namespace, and `NodeDevice` rows are keyed by it — so a subgroup id in the
    // path would otherwise report an account for the namespace beside a device
    // lookup against the subgroup, which misses an existing enrollment and answers
    // "no device" for a node that has one.
    let namespace =
        match calimero_governance_store::NamespaceRepository::new(store).resolve(&group_id) {
            Ok(namespace) => namespace,
            Err(err) => {
                error!(error = ?err, "Failed to resolve the namespace");
                return ApiError {
                    status_code: StatusCode::NOT_FOUND,
                    message: format!("No namespace found for '{namespace_id_str}'"),
                }
                .into_response();
            }
        };

    let account = match calimero_governance_store::account_for_group(store, &namespace) {
        Ok(account) => account,
        Err(err) => {
            error!(error = ?err, "Failed to resolve the node's account");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to resolve this node's account".to_owned(),
            }
            .into_response();
        }
    };

    // A missing device is a real answer; a failed READ is not, and reporting the two
    // the same way would tell an operator "not enrolled" when the truth is "could
    // not look".
    let device = match calimero_governance_store::NodeDeviceRepository::new(store).get(&namespace) {
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

    ApiResponse {
        payload: NamespaceAccountApiResponse {
            data: NamespaceAccountApiResponseData {
                account_id: hex::encode(account.as_bytes()),
                namespace_id: hex::encode(namespace.to_bytes()),
                device_id: device,
            },
        },
    }
    .into_response()
}
