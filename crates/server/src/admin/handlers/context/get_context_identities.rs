use std::sync::Arc;

use axum::extract::{Path, Request};
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::GetContextIdentitiesResponse;
use futures_util::TryStreamExt;
use reqwest::StatusCode;
use tokio::task;

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
    req: Request,
) -> impl IntoResponse {
    let owned = req.uri().path().ends_with("identities-owned");

    // Get the context info
    let context = match state.ctx_client.get_context(&context_id) {
        Ok(Some(context)) => context,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Context not found".into(),
            }
            .into_response()
        }
        Err(err) => return parse_api_error(err).into_response(),
    };

    // Clone what we need
    let id = context.id;
    let ctx_client = state.ctx_client.clone();

    // Process the stream in a blocking task to handle non-Send types
    let result = task::spawn_blocking(move || {
        // Create a runtime for async operations inside the blocking task
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        // Execute the async operations inside the runtime
        rt.block_on(async {
            // Get the stream
            let stream = ctx_client.context_members(&id, Some(owned)).await;

            // Collect identities - this happens inside our isolated runtime
            let identities: Vec<_> = stream.try_collect().await.unwrap_or_else(|_| Vec::new());

            identities
        })
    })
    .await;

    match result {
        Ok(identities) => ApiResponse {
            payload: GetContextIdentitiesResponse::new(identities),
        }
        .into_response(),
        Err(_) => ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Failed to process identities".into(),
        }
        .into_response(),
    }
}
