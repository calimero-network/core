use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{JoinContextRequest, JoinContextResponse};
use tracing::error;

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

// Placeholder Context ID used for storing unassigned keys
const PLACEHOLDER_CONTEXT_ID_BYTES: [u8; 32] = [0; 32];

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(JoinContextRequest {
        public_key,
        invitation_payload,
    }): Json<JoinContextRequest>,
) -> impl IntoResponse {
    // 1. Extract invitee_id from payload and verify it matches the provided public_key
    let invitee_id_result = invitation_payload.parts().map(|(_, id, _, _, _)| id);
    let _invitee_id = match invitee_id_result {
        Ok(id) => {
            if id != public_key {
                error!("Public key in request does not match invitee ID in payload");
                return ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "Public key mismatch".into(),
                }
                .into_response();
            }
            id
        }
        Err(e) => {
            error!("Failed to parse invitation payload: {}", e);
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid invitation payload".into(),
            }
            .into_response();
        }
    };

    // 2. Retrieve the pre-stored private key using the ContextManager method
    let private_key_result = (|| {
        let placeholder_context_id = ContextId::from(PLACEHOLDER_CONTEXT_ID_BYTES);

        let stored_identity = state
            .ctx_manager
            .get_identity_value(placeholder_context_id, public_key)?
            .ok_or_else(|| {
                eyre::eyre!(
                    "Pre-stored private key not found for public key: {}",
                    public_key
                )
            })?;

        let private_key = stored_identity
            .private_key
            .ok_or_else(|| eyre::eyre!("Stored identity value is missing private key"))?;

        // 3. Delete the temporary entry using the ContextManager method
        state
            .ctx_manager
            .delete_identity_value(placeholder_context_id, public_key)?;

        Ok::<_, eyre::Report>(private_key.into())
    })();

    let private_key = match private_key_result {
        Ok(pk) => pk,
        Err(e) => {
            error!("Failed to retrieve or delete pre-stored key: {}", e);
            // Use a specific error or a generic one
            let api_error = if e.to_string().contains("Pre-stored private key not found") {
                ApiError {
                    status_code: StatusCode::BAD_REQUEST, // Or NOT_FOUND?
                    message: "Identity generation process not completed or key expired".into(),
                }
            } else {
                ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "Failed to process join request".into(),
                }
            };
            return api_error.into_response();
        }
    };

    // 4. Call the actual join context logic with the retrieved private key
    let result = state
        .ctx_manager
        .join_context(private_key, invitation_payload)
        .await
        .map_err(parse_api_error);

    match result {
        Ok(result) => ApiResponse {
            payload: JoinContextResponse::new(result),
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
