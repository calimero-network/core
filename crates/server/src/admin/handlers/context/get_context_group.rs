use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::GetGroupForContextRequest;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::GetContextGroupApiResponse;
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    info!(%context_id, "Getting group for context");

    let result = state
        .ctx_client
        .get_group_for_context(GetGroupForContextRequest { context_id })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(group_id) => {
            info!(%context_id, "Context group retrieved successfully");
            ApiResponse {
                payload: GetContextGroupApiResponse {
                    data: group_id.map(|g| hex::encode(g.to_bytes())),
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(%context_id, error=?err, "Failed to get group for context");
            err.into_response()
        }
    }
}
