use std::sync::Arc;

use axum::extract::{Path, Request};
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{ContextAlias, GetContextAliasesResponse};
use reqwest::StatusCode;

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
    _req: Request,
) -> impl IntoResponse {
    let context = match state.ctx_client.get_context(&context_id) {
        Ok(Some(ctx)) => ctx,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Context not found".into(),
            }
            .into_response()
        }
        Err(err) => return parse_api_error(err).into_response(),
    };

    let result = state.node_client.list_aliases(Some(context.id));
    let aliases_raw = match result {
        Ok(a) => a,
        Err(err) => return parse_api_error(err).into_response(),
    };

    let aliases: Vec<ContextAlias> = aliases_raw
        .into_iter()
        .map(|(alias, identity, _scope)| ContextAlias { alias, identity })
        .collect();

    ApiResponse {
        payload: GetContextAliasesResponse::new(aliases),
    }
    .into_response()
}
