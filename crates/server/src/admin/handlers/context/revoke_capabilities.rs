use std::sync::Arc;

use axum::extract::{Json, Path};
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_config::types::{Capability, ContextIdentity};
use calimero_primitives::context::ContextId;
use serde::Deserialize;

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

#[derive(Deserialize)]
pub struct RevokeCapabilitiesRequest {
    pub capabilities: Vec<(ContextIdentity, Capability)>,
}

pub async fn handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(request): Json<RevokeCapabilitiesRequest>,
) -> impl IntoResponse {
    let context = match state.ctx_manager.get_context(&context_id) {
        Ok(Some(context)) => context,
        Ok(None) => {
            return parse_api_error(eyre::eyre!("Context not found")).into_response();
        }
        Err(err) => {
            return parse_api_error(err).into_response();
        }
    };

    match state.ctx_manager.revoke_capabilities(context.id, &request.capabilities).await {
        Ok(_) => ApiResponse { payload: () }.into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}