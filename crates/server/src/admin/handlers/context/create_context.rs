use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::{
    CreateContextRequest, CreateContextResponse, CreateContextResponseData,
};
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<CreateContextRequest>,
) -> impl IntoResponse {
    info!(application_id=%req.application_id, "Creating context");

    let identity_secret = req.identity_secret.map(PrivateKey::from);

    let result = state
        .ctx_client
        .create_context(
            req.protocol,
            &req.application_id,
            identity_secret,
            req.initialization_params,
            req.context_seed.map(Into::into),
            req.group_id,
        )
        .await
        .map_err(parse_api_error);

    match result {
        Ok(context) => {
            info!(context_id=%context.context_id, "Context created successfully");
            ApiResponse {
                payload: CreateContextResponse {
                    data: CreateContextResponseData {
                        context_id: context.context_id,
                        member_public_key: context.identity,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(error=?err, application_id=%req.application_id, "Failed to create context");
            err.into_response()
        }
    }
}
