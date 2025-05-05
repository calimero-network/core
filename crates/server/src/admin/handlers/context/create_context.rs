use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{CreateContextRequest, CreateContextResponse};
use tokio::sync::oneshot;

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<CreateContextRequest>,
) -> impl IntoResponse {
    let (tx, rx) = oneshot::channel();

    let result = state
        .ctx_manager
        .create_context(
            req.protocol,
            req.context_seed.map(Into::into),
            req.application_id,
            None,
            req.initialization_params,
            tx,
        )
        .map_err(parse_api_error);

    if let Err(err) = result {
        return err.into_response();
    }

    let Ok(result) = rx.await else {
        return "internal error".into_response();
    };

    match result {
        Ok((context_id, member_public_key)) => ApiResponse {
            payload: CreateContextResponse::new(context_id, member_public_key),
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}
