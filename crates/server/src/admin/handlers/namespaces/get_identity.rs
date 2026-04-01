use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_config::types::ContextGroupId;
use calimero_context_primitives::group::GetNamespaceIdentityRequest;
use calimero_server_primitives::admin::NamespaceIdentityApiResponse;
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_namespace_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(namespace_id=%namespace_id_str, "Getting namespace identity");

    match state
        .ctx_client
        .get_namespace_identity(GetNamespaceIdentityRequest { group_id })
        .await
    {
        Ok(Some((ns_id, pk))) => ApiResponse {
            payload: NamespaceIdentityApiResponse {
                namespace_id: hex::encode(ns_id.to_bytes()),
                public_key: pk.to_string(),
            },
        }
        .into_response(),
        Ok(None) => ApiError {
            status_code: StatusCode::NOT_FOUND,
            message: "No namespace identity found".to_owned(),
        }
        .into_response(),
        Err(err) => {
            error!(error=?err, "Failed to get namespace identity");
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to get namespace identity".to_owned(),
            }
            .into_response()
        }
    }
}

fn parse_namespace_id(s: &str) -> Result<ContextGroupId, ApiError> {
    let bytes = hex::decode(s).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid namespace id: expected hex-encoded 32 bytes".into(),
    })?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Invalid namespace id: must be exactly 32 bytes".into(),
    })?;
    Ok(ContextGroupId::from(arr))
}
