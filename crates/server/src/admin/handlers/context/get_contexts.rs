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
    // Process the stream in a blocking task
    let state_clone = state.clone();
    let result = task::spawn_blocking(move || {
        // Create a new runtime for the async code inside our blocking task
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
            
        rt.block_on(async {
            // Get the stream
            let stream = state_clone.ctx_client.get_contexts(None).await;
            
            // Collect context IDs
            let context_ids = match stream.try_collect::<Vec<_>>().await {
                Ok(ids) => ids,
                Err(err) => return Err(err),
            };
            
            // Get context details for each ID
            let mut contexts = Vec::new();
            for context_id in context_ids {
                if let Ok(Some(context)) = state_clone.ctx_client.get_context(&context_id) {
                    contexts.push(context);
                }
            }
            
            Ok(contexts)
        })
    }).await;
    
    match result {
        Ok(Ok(contexts)) => ApiResponse {
            payload: GetContextsResponse::new(contexts),
        }.into_response(),
        Ok(Err(err)) => parse_api_error(err).into_response(),
        Err(_) => ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Failed to process contexts".into(),
        }.into_response(),
    }
}
