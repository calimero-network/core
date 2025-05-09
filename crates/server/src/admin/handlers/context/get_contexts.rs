use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::GetContextsResponse;
use futures_util::TryStreamExt;
use reqwest::StatusCode;
use tokio::task;

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    // fixme! Remove the need for special accommodations with blocking task and runtime
    let state_clone = state.clone();
    let result = task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let stream = state_clone.ctx_client.get_contexts(None).await;

            // fixme! Improve error handling for the stream collection
            let context_ids = match stream.try_collect::<Vec<_>>().await {
                Ok(ids) => ids,
                Err(err) => return Err(err),
            };

            let mut contexts = Vec::new();
            for context_id in context_ids {
                if let Some(context) = state_clone.ctx_client.get_context(&context_id)? {
                    contexts.push(context);
                }
            }

            Ok(contexts)
        })
    })
    .await;

    match result {
        Ok(Ok(contexts)) => ApiResponse {
            payload: GetContextsResponse::new(contexts),
        }
        .into_response(),
        Ok(Err(err)) => parse_api_error(err).into_response(),
        Err(_) => ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Failed to process contexts".into(),
        }
        .into_response(),
    }
}
