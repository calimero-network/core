//! Handler for specialized node invitation.
//!
//! This endpoint broadcasts a specialized node discovery request to the global invite topic,
//! allowing specialized nodes (e.g., read-only TEE nodes) to respond with verification
//! and receive invitations.

use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{
    InviteSpecializedNodeRequest, InviteSpecializedNodeResponse,
};
use futures_util::TryStreamExt;
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<InviteSpecializedNodeRequest>,
) -> impl IntoResponse {
    info!(context_id=%req.context_id, "Initiating specialized node invitation");

    // Resolve inviter_id - use provided or get default identity for context
    let inviter_id = match req.inviter_id {
        Some(id) => id,
        None => {
            // Get the first owned identity for this context
            let stream = state
                .ctx_client
                .get_context_members(&req.context_id, Some(true)); // true = owned only

            match stream.map_ok(|(id, _)| id).try_collect::<Vec<_>>().await {
                Ok(identities) => {
                    if let Some(first_identity) = identities.into_iter().next() {
                        first_identity
                    } else {
                        error!(context_id=%req.context_id, "No owned identities found for context");
                        return ApiError {
                            status_code: axum::http::StatusCode::BAD_REQUEST,
                            message: "No owned identities found for context".to_owned(),
                        }
                        .into_response();
                    }
                }
                Err(err) => {
                    error!(error=?err, "Failed to get context identities");
                    return parse_api_error(err).into_response();
                }
            }
        }
    };

    // Broadcast specialized node invite and register pending invite
    let result = state
        .node_client
        .broadcast_specialized_node_invite(req.context_id, inviter_id)
        .await;

    match result {
        Ok(nonce) => {
            let nonce_hex = hex::encode(nonce);
            info!(
                context_id=%req.context_id,
                %inviter_id,
                %nonce_hex,
                "Specialized node invite discovery broadcast successfully"
            );
            ApiResponse {
                payload: InviteSpecializedNodeResponse::new(nonce_hex),
            }
            .into_response()
        }
        Err(err) => {
            error!(error=?err, "Failed to broadcast specialized node invite discovery");
            parse_api_error(err).into_response()
        }
    }
}
