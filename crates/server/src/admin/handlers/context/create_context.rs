use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::{
    CreateContextRequest, CreateContextResponse, CreateContextResponseData,
};
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<CreateContextRequest>,
) -> impl IntoResponse {
    info!(application_id=%req.application_id, "Creating context");

    let group_id = match req.group_id.as_deref().map(parse_group_id).transpose() {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let identity_secret = match req
        .identity_secret
        .as_deref()
        .map(decode_identity_secret)
        .transpose()
    {
        Ok(key) => key,
        Err(err) => return err.into_response(),
    };

    let result = state
        .ctx_client
        .create_context(
            "local".to_owned(),
            &req.application_id,
            identity_secret,
            req.initialization_params,
            req.context_seed.map(Into::into),
            group_id,
            req.alias,
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

fn parse_group_id(hex_str: &str) -> Result<ContextGroupId, ApiError> {
    let bytes = hex::decode(hex_str).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid group_id: expected hex-encoded 32 bytes".into(),
    })?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid group_id: must be exactly 32 bytes".into(),
    })?;
    Ok(ContextGroupId::from(arr))
}

fn decode_identity_secret(hex_str: &str) -> Result<PrivateKey, ApiError> {
    let bytes = hex::decode(hex_str).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid identity_secret: expected hex-encoded 32 bytes".into(),
    })?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid identity_secret: must be exactly 32 bytes".into(),
    })?;
    Ok(PrivateKey::from(arr))
}
