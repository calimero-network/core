use std::pin::pin;
use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::GetContextsResponse;
use futures_util::TryStreamExt;

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    let context_ids = state.ctx_client.get_contexts(None);

    let mut context_ids = pin!(context_ids);

    let mut contexts = Vec::new();

    while let Some(context_id) = context_ids.try_next().await.transpose() {
        let context_id = match context_id {
            Ok(id) => id,
            Err(err) => return parse_api_error(err).into_response(),
        };

        match state.ctx_client.get_context(&context_id) {
            Ok(None) => {}
            Ok(Some(context)) => contexts.push(context),
            Err(err) => return parse_api_error(err).into_response(),
        }
    }

    ApiResponse {
        payload: GetContextsResponse::new(contexts),
    }
    .into_response()
}
