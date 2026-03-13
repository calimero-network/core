use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::GetContextAllowlistRequest;
use calimero_server_primitives::admin::GetContextAllowlistApiResponse;
use tracing::{error, info};

use super::{parse_context_id, parse_group_id};
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path((group_id_str, context_id_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let context_id = match parse_context_id(&context_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, context_id=%context_id_str, "Getting context allowlist");

    let result = state
        .ctx_client
        .get_context_allowlist(GetContextAllowlistRequest {
            group_id,
            context_id,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(members) => {
            info!(group_id=%group_id_str, context_id=%context_id_str, count=members.len(), "Got context allowlist");
            ApiResponse {
                payload: GetContextAllowlistApiResponse { data: members },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, context_id=%context_id_str, error=?err, "Failed to get context allowlist");
            err.into_response()
        }
    }
}
