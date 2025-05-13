use std::sync::Arc;

use axum::extract::{Json, Path};
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_config::repr::Repr;
use calimero_context_config::types::{Capability, ContextIdentity};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use serde::Deserialize;

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

#[derive(Deserialize, Debug)]
pub struct RevokeCapabilitiesRequest {
    pub capabilities: Vec<(Repr<ContextIdentity>, Capability)>,
    pub signer_id: PublicKey,
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

    let capabilities_to_revoke: Vec<(ContextIdentity, Capability)> = request
        .capabilities
        .into_iter()
        .map(|(identity_repr, capability)| (*identity_repr, capability))
        .collect();

    match state
        .ctx_manager
        .revoke_capabilities(context.id, request.signer_id, &capabilities_to_revoke)
        .await
    {
        Ok(_) => ApiResponse { payload: () }.into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}
