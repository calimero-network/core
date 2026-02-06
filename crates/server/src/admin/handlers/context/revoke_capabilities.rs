use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_config::types::Capability;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::validation::{
    helpers::validate_collection_size, Validate, ValidationError, MAX_CAPABILITIES_COUNT,
};
use eyre::bail;
use serde::Deserialize;
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

#[derive(Deserialize, Debug)]
pub struct RevokeCapabilitiesRequest {
    pub capabilities: Vec<(PublicKey, Capability)>,
    pub signer_id: PublicKey,
}

impl Validate for RevokeCapabilitiesRequest {
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
    ValidatedJson(request): ValidatedJson<RevokeCapabilitiesRequest>,
) -> impl IntoResponse {
    info!(context_id=%context_id, signer_id=%request.signer_id, count=%request.capabilities.len(), "Revoking capabilities");

    let res = async {
        let Some(config_client) = state.ctx_client.context_config(&context_id)? else {
            bail!("context '{}' does not exist", context_id);
        };

        let external_client = state
            .ctx_client
            .external_client(&context_id, &config_client)?;

        external_client
            .config()
            .revoke(&request.signer_id, &request.capabilities)
            .await
    };
    match res.await {
        Ok(_) => {
            info!(context_id=%context_id, signer_id=%request.signer_id, "Capabilities revoked successfully");
            ApiResponse { payload: () }.into_response()
        }
        Err(err) => {
            error!(context_id=%context_id, signer_id=%request.signer_id, error=?err, "Failed to revoke capabilities");
            parse_api_error(err).into_response()
        }
    }
}
