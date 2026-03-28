use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_config::types::Capability;
use calimero_context_primitives::group::GrantContextCapabilitiesRequest;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::validation::{
    helpers::validate_collection_size, Validate, ValidationError, MAX_CAPABILITIES_COUNT,
};
use serde::Deserialize;
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

#[derive(Deserialize, Debug)]
pub struct GrantCapabilitiesRequest {
    pub capabilities: Vec<(PublicKey, Capability)>,
    pub signer_id: PublicKey,
}

impl Validate for GrantCapabilitiesRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        if let Some(e) =
            validate_collection_size(&self.capabilities, "capabilities", MAX_CAPABILITIES_COUNT)
        {
            errors.push(e);
        }

        errors
    }
}

pub async fn handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(request): ValidatedJson<GrantCapabilitiesRequest>,
) -> impl IntoResponse {
    info!(context_id=%context_id, signer_id=%request.signer_id, count=%request.capabilities.len(), "Granting capabilities");

    let caps: Vec<(PublicKey, u8)> = request
        .capabilities
        .iter()
        .map(|(pk, cap)| (*pk, cap.as_bit()))
        .collect();

    let result = state
        .ctx_client
        .grant_context_capabilities(GrantContextCapabilitiesRequest {
            context_id,
            capabilities: caps,
            signer_id: request.signer_id,
        })
        .await;

    match result {
        Ok(()) => {
            info!(context_id=%context_id, signer_id=%request.signer_id, "Capabilities granted successfully");
            ApiResponse { payload: () }.into_response()
        }
        Err(err) => {
            error!(context_id=%context_id, signer_id=%request.signer_id, error=?err, "Failed to grant capabilities");
            parse_api_error(err).into_response()
        }
    }
}
