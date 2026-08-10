use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::{
    RegisterGroupSigningKeyApiRequest, RegisterGroupSigningKeyApiResponse,
    RegisterGroupSigningKeyApiResponseData,
};
use calimero_store::key::{GroupMember, GroupSigningKey, GroupSigningKeyValue};
use reqwest::StatusCode;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<RegisterGroupSigningKeyApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let private_key_bytes: [u8; 32] = match hex::decode(&req.signing_key)
        .map_err(|_| ())
        .and_then(|v| v.try_into().map_err(|_| ()))
    {
        Ok(bytes) => bytes,
        Err(()) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid signing_key: expected hex-encoded 32 bytes".into(),
            }
            .into_response();
        }
    };

    let private_key = PrivateKey::from(private_key_bytes);
    let public_key = private_key.public_key();

    // Verify the identity is a member of this group. The row names the account
    // the key acts as; a key bound to none holds no row, which reads the same as
    // "not a member" below.
    let handle = state.store.handle();
    let Some(account) = calimero_governance_store::member_account_in_namespace(
        &state.store,
        &group_id,
        &public_key,
    )
    .ok()
    .flatten() else {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Identity is not a member of this group".into(),
        }
        .into_response();
    };
    let member_key = GroupMember::new(group_id.to_bytes(), account);
    match handle.has(&member_key) {
        Ok(true) => {}
        Ok(false) => {
            return ApiError {
                status_code: StatusCode::FORBIDDEN,
                message: "Identity derived from signing key is not a member of this group".into(),
            }
            .into_response();
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to check group membership");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to check group membership: {err}"),
            }
            .into_response();
        }
    }
    drop(handle);

    // Store the signing key
    let mut handle = state.store.handle();
    let store_key = GroupSigningKey::new(group_id.to_bytes(), public_key);
    if let Err(err) = handle.put(
        &store_key,
        &GroupSigningKeyValue {
            private_key: private_key_bytes,
        },
    ) {
        error!(group_id=%group_id_str, error=?err, "Failed to store signing key");
        return ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("Failed to store signing key: {err}"),
        }
        .into_response();
    }

    info!(group_id=%group_id_str, %public_key, "Group signing key registered");

    ApiResponse {
        payload: RegisterGroupSigningKeyApiResponse {
            data: RegisterGroupSigningKeyApiResponseData { public_key },
        },
    }
    .into_response()
}
