//! Handler for specialized node invitation.
//!
//! This endpoint broadcasts a specialized node discovery request to the global invite topic,
//! allowing specialized nodes (e.g., read-only TEE nodes) to respond with verification
//! and receive invitations.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{
    InviteSpecializedNodeRequest, InviteSpecializedNodeResponse,
};
use futures_util::TryStreamExt;
use tracing::{error, info, warn};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

/// Minimum interval between specialized-node invite broadcasts for the same
/// context. Each broadcast publishes to the *global* invite topic, so without a
/// throttle an authenticated caller could flood the network. Rate-limiting is
/// keyed by context (only real contexts with owned identities reach this point,
/// so the map cannot be grown with arbitrary ids).
const INVITE_MIN_INTERVAL: Duration = Duration::from_secs(5);

fn last_broadcasts() -> &'static Mutex<HashMap<ContextId, Instant>> {
    static LAST: OnceLock<Mutex<HashMap<ContextId, Instant>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(HashMap::new()))
}

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<InviteSpecializedNodeRequest>,
) -> impl IntoResponse {
    info!(context_id=%req.context_id, "Initiating specialized node invitation");

    // The invite is stamped with an `inviter_id`. Resolve the set of identities
    // this node owns in the context and require the inviter to be one of them:
    // a caller must not be able to broadcast an invite spoofed as an arbitrary
    // identity it does not control. When no `inviter_id` is supplied, default to
    // the node's first owned identity for the context.
    let owned: Vec<_> = match state
        .ctx_client
        .get_context_members(&req.context_id, Some(true)) // true = owned only
        .map_ok(|(id, _)| id)
        .try_collect()
        .await
    {
        Ok(identities) => identities,
        Err(err) => {
            error!(error=?err, "Failed to get context identities");
            return parse_api_error(err).into_response();
        }
    };

    let inviter_id = match req.inviter_id {
        Some(id) => {
            if !owned.contains(&id) {
                warn!(context_id=%req.context_id, %id, "rejecting specialized invite: inviter_id is not an owned identity of the context");
                return ApiError {
                    status_code: StatusCode::FORBIDDEN,
                    message: "inviter_id is not an owned identity of this context".to_owned(),
                }
                .into_response();
            }
            id
        }
        None => match owned.into_iter().next() {
            Some(first_identity) => first_identity,
            None => {
                error!(context_id=%req.context_id, "No owned identities found for context");
                return ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "No owned identities found for context".to_owned(),
                }
                .into_response();
            }
        },
    };

    // Throttle broadcasts per context to bound global-topic traffic.
    {
        let now = Instant::now();
        let mut last = last_broadcasts().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(prev) = last.get(&req.context_id) {
            if now.duration_since(*prev) < INVITE_MIN_INTERVAL {
                warn!(context_id=%req.context_id, "throttling specialized invite broadcast");
                return ApiError {
                    status_code: StatusCode::TOO_MANY_REQUESTS,
                    message: "Specialized node invites for this context are rate limited"
                        .to_owned(),
                }
                .into_response();
            }
        }
        let _ = last.insert(req.context_id, now);
    }

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
