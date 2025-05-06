use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::GetContextsResponse;

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    // todo! experiment with Interior<Store>: WriteLayer<Interior>
    let contexts = state.ctx_client.get_contexts(None).map_err(parse_api_error);

    match contexts {
        Ok(contexts) => ApiResponse {
            payload: GetContextsResponse::new(contexts),
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
