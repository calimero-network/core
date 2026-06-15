use core::str::FromStr;
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::ResyncContextRequest;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{ResyncContextApiRequest, ResyncContextApiResponse};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

/// `POST /admin-api/contexts/{context_id}/resync` — operator-facing full-state
/// resync of a stranded context. Destructive: pass `{"force": true}` to discard
/// local DAG heads and pull current state from a peer.
pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Path(context_id): Path<String>,
    ValidatedJson(req): ValidatedJson<ResyncContextApiRequest>,
) -> impl IntoResponse {
    let context_id = match ContextId::from_str(&context_id) {
        Ok(id) => id,
        Err(err) => {
            error!(context_id=%context_id, error=?err, "Invalid context ID format");
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid context id".into(),
            }
            .into_response();
        }
    };

    info!(%context_id, force=req.force, "Resyncing context");

    let result = state
        .ctx_client
        .resync_context(ResyncContextRequest {
            context_id,
            force: req.force,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(response) => ApiResponse {
            payload: ResyncContextApiResponse {
                context_id: hex::encode(response.context_id.as_ref()),
                resync_started: response.resync_started,
            },
        }
        .into_response(),
        Err(err) => {
            error!(%context_id, error=?err, "Failed to resync context");
            err.into_response()
        }
    }
}
