use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::GetContextResponse;
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    info!(context_id=%context_id, "Getting context");

    // todo! experiment with Interior<Store>: WriteLayer<Interior>
    let context = state
        .ctx_client
        .get_context(&context_id)
        .map_err(|err| parse_api_error(err).into_response());

    #[expect(clippy::option_if_let_else, reason = "Clearer here")]
    match context {
        Ok(ctx) => match ctx {
            Some(context) => {
                info!(context_id=%context_id, "Context retrieved successfully");
                ApiResponse {
                    payload: GetContextResponse { data: context },
                }
                .into_response()
            }
            None => {
                info!(context_id=%context_id, "Context not found");
                ApiError {
                    status_code: StatusCode::NOT_FOUND,
                    message: "Context not found".into(),
                }
                .into_response()
            }
        },
        Err(err) => {
            error!(context_id=%context_id, "Failed to get context");
            err.into_response()
        }
    }
}
