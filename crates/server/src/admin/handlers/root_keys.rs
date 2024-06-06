use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::identity::RootKey;
use calimero_server_primitives::admin::{AddPublicKeyRequest, IntermediateAddPublicKeyRequest};
use calimero_store::Store;
use chrono::Utc;
use serde::Serialize;
use tracing::info;

use super::add_client_key::transform_request;
use crate::admin::service::{parse_api_error, AdminState, ApiError, ApiResponse};
use crate::admin::storage::root_key::{add_root_key, get_root_keys};
use crate::admin::utils::auth::{validate_challenge, validate_root_key_exists};

#[derive(Debug, Serialize)]
struct CreateRootKeyResponse {
    data: String,
}

pub async fn create_root_key_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(intermediate_req): Json<IntermediateAddPublicKeyRequest>,
) -> impl IntoResponse {
    let response = transform_request(intermediate_req)
        .and_then(|req| check_if_first_root_key(req, &state.store))
        .and_then(|req| validate_challenge(req, &state.keypair))
        .and_then(|req| store_root_key(req, &state.store))
        .map_or_else(
            |err| err.into_response(),
            |_| {
                let data: String = "Root key stored".to_string();
                ApiResponse {
                    payload: CreateRootKeyResponse { data },
                }
                .into_response()
            },
        );

    response
}

/**
 * If first root key then don't validate wallet signature
 */
fn check_if_first_root_key(
    req: AddPublicKeyRequest,
    store: &Store,
) -> Result<AddPublicKeyRequest, ApiError> {
    let root_keys = get_root_keys(&store).map_err(|e| parse_api_error(e))?;
    if root_keys.is_empty() {
        println!("First root key");
        Ok(req)
    } else {
        println!("Not first root key {:?}", root_keys);
        validate_root_key_exists(req, &store)
    }
}

fn store_root_key(
    req: AddPublicKeyRequest,
    store: &Store,
) -> Result<AddPublicKeyRequest, ApiError> {
    let root_key = RootKey {
        signing_key: req.payload.message.public_key.clone(),
        wallet_type: req.wallet_metadata.wallet_type,
        created_at: Utc::now().timestamp_millis() as u64,
    };
    add_root_key(&store, root_key).map_err(|e| parse_api_error(e))?;

    info!("Root key stored successfully.");
    Ok(req)
}
