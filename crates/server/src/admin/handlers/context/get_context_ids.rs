use std::pin::pin;
use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::GetContextsResponse;
use futures_util::TryStreamExt;
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    info!("Listing contexts");

    let context_ids = state.ctx_client.get_context_ids(None);
    let mut context_ids = pin!(context_ids);
    let mut contexts = Vec::new();

    while let Some(context_id) = context_ids.try_next().await.transpose() {
        let context_id = match context_id {
            Ok(id) => id,
            Err(err) => {
                error!(error=?err, "Failed to get context IDs");
                return parse_api_error(err).into_response();
            }
        };

        match state.ctx_client.get_context(&context_id) {
            Ok(None) => {}
            Ok(Some(context)) => contexts.push(context),
            Err(err) => {
                error!(context_id=%context_id, error=?err, "Failed to get context");
                return parse_api_error(err).into_response();
            }
        }
    }

    info!(count=%contexts.len(), "Contexts listed successfully");

    ApiResponse {
        payload: GetContextsResponse::new(contexts),
    }
    .into_response()
}
