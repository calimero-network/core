use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::SyncContextResponse;

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    context_id: Option<Path<ContextId>>,
) -> impl IntoResponse {
    let context_id = context_id.map(|Path(c)| c);

    let result = state.node_client.sync(context_id.as_ref()).await;

    match result {
        Ok(()) => ApiResponse {
            payload: SyncContextResponse::new(),
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}
