use core::str::FromStr;
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{
    DeleteContextApiRequest, DeleteContextResponse, DeletedContextResponseData,
};
use reqwest::StatusCode;
use tower_sessions::Session;
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path(context_id): Path<String>,
    _session: Session,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    body: Option<Json<DeleteContextApiRequest>>,
) -> impl IntoResponse {
    let context_id_result = match ContextId::from_str(&context_id) {
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

    // Prefer the authenticated identity over the caller-supplied requester to
    // prevent authorization bypass via a spoofed public key in the request body.
    let requester = auth_key
        .map(|Extension(k)| k.0)
        .or_else(|| body.and_then(|Json(req)| req.requester));

    info!(context_id=%context_id_result, "Deleting context");

    // todo! experiment with Interior<Store>: WriteLayer<Interior>
    let result = state
        .ctx_client
        .delete_context(&context_id_result, requester)
        .await
        .map_err(parse_api_error);

    match result {
        Ok(result) => {
            info!(context_id=%context_id_result, deleted=%result.deleted, "Context deletion completed");
            ApiResponse {
                payload: DeleteContextResponse {
                    data: DeletedContextResponseData {
                        is_deleted: result.deleted,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(context_id=%context_id_result, error=?err, "Failed to delete context");
            err.into_response()
        }
    }
}
