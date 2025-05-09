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

    // fixme! Remove the need for special accommodations with blocking task and runtime
    let result = task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let stream = state
                .ctx_client
                .context_members(&context.id, Some(owned))
                .await;

            // fixme! Improve error handling for the stream collection
            let identities: Vec<_> = stream.try_collect().await.unwrap_or_else(|_| Vec::new());

            identities
        })
    })
    .await;

    match result {
        Ok(identities) => ApiResponse {
            payload: GetContextIdentitiesResponse::new(
                identities.into_iter().map(|(id, _)| id).collect(),
            ),
        }
        .into_response(),
        Err(_) => ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Failed to process identities".into(),
        }
        .into_response(),
    }
}
