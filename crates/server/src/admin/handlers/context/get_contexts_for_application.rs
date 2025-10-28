use std::pin::pin;
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::GetContextsResponse;
use futures_util::TryStreamExt;
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Path(application_id): Path<ApplicationId>,
) -> impl IntoResponse {
    info!(application_id=%application_id, "Getting contexts for application");

    let context_ids = state.ctx_client.get_context_ids(None);
    let mut context_ids = pin!(context_ids);
    let mut contexts = Vec::new();

    while let Some(context_id) = context_ids.try_next().await.transpose() {
        let context_id = match context_id {
            Ok(id) => id,
            Err(err) => {
                error!(application_id=%application_id, error=?err, "Failed to get context IDs");
                return parse_api_error(err).into_response();
            }
        };

        match state.ctx_client.get_context(&context_id) {
            Ok(Some(context)) => {
                // Filter contexts by application_id
                if context.application_id == application_id {
                    contexts.push(context);
                }
            }
            Ok(None) => {
                // Context doesn't exist, skip
                continue;
            }
            Err(err) => {
                error!(application_id=%application_id, context_id=%context_id, error=?err, "Failed to get context");
                continue;
            }
        }
    }

    info!(application_id=%application_id, contexts_count=%contexts.len(), "Retrieved contexts for application");
    ApiResponse {
        payload: GetContextsResponse::new(contexts),
    }
    .into_response()
}
