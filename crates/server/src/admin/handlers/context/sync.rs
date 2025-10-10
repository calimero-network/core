use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::SyncContextResponse;
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    context_id: Option<Path<ContextId>>,
) -> impl IntoResponse {
    let context_id = context_id.map(|Path(c)| c);

    if let Some(ref ctx_id) = context_id {
        info!(context_id=%ctx_id, "Syncing context");
    } else {
        info!("Syncing all contexts");
    }

    let result = state.node_client.sync(context_id.as_ref(), None).await;

    match result {
        Ok(()) => {
            if let Some(ref ctx_id) = context_id {
                info!(context_id=%ctx_id, "Context sync completed successfully");
            } else {
                info!("All contexts sync completed successfully");
            }
            ApiResponse {
                payload: SyncContextResponse::new(),
            }
            .into_response()
        }
        Err(err) => {
            if let Some(ref ctx_id) = context_id {
                error!(context_id=%ctx_id, error=?err, "Failed to sync context");
            } else {
                error!(error=?err, "Failed to sync contexts");
            }
            parse_api_error(err).into_response()
        }
    }
}
