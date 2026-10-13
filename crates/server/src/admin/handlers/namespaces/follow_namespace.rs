//! `POST /admin-api/namespaces/:namespace_id/follow` — follow a namespace this
//! node's account was added to, with no invitation.
//!
//! The admin who added the account is the authority; this route only asks the
//! namespace's anchors for the key and the governance this node is missing.
//! Every decision is the context handler's (`ContextClient::follow_group`); the
//! route parses, forwards, and maps a `FollowRefused` through `parse_api_error`.

use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::FollowGroupRequest;
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::{
    FollowNamespaceApiRequest, FollowNamespaceApiResponse, FollowNamespaceApiResponseData,
};
use reqwest::StatusCode;
use tracing::{debug, error, info};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<FollowNamespaceApiRequest>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    // `ValidatedJson` already refused anything but 64 hex, so these two only
    // turn the checked strings into their types.
    let application_id = match req.application_id.parse::<ApplicationId>() {
        Ok(id) => id,
        Err(err) => return bad_request(format!("invalid applicationId: {err}")),
    };
    let bytecode_id = match req
        .bytecode_id
        .as_deref()
        .map(parse_bytecode_id)
        .transpose()
    {
        Ok(id) => id,
        Err(message) => return bad_request(message),
    };

    info!(
        namespace_id = %namespace_id_str,
        peers = req.peer_addrs.len(),
        "Following namespace"
    );

    let result = state
        .ctx_client
        .follow_group(FollowGroupRequest {
            namespace_id,
            peer_addrs: req.peer_addrs,
            application_id,
            bytecode_id,
        })
        .await
        // `ContextError::FollowRefused` is typed: `parse_api_error` answers a
        // membership refusal with `403` and an unanswered or unfinished follow
        // with a retryable `503`.
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            let namespace_id_hex = hex::encode(resp.namespace_id.to_bytes());
            debug!(
                namespace_id = %namespace_id_hex,
                member = %resp.member_identity,
                key_ready = resp.key_ready,
                "Followed namespace"
            );
            ApiResponse {
                payload: FollowNamespaceApiResponse {
                    data: FollowNamespaceApiResponseData {
                        namespace_id: namespace_id_hex,
                        member_account: resp.member_account.to_string(),
                        key_ready: resp.key_ready,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(namespace_id = %namespace_id_str, error = ?err, "Failed to follow namespace");
            err.into_response()
        }
    }
}

fn parse_bytecode_id(hex_id: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(hex_id).map_err(|err| format!("invalid bytecodeId: {err}"))?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| format!("invalid bytecodeId: expected 32 bytes, got {}", bytes.len()))
}

fn bad_request(message: String) -> axum::response::Response {
    ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message,
    }
    .into_response()
}
