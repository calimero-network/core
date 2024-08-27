use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::identity::{ClientKey, WalletType};
use calimero_server_primitives::admin::{
    AddPublicKeyRequest, EthSignatureMessageMetadata, IntermediateAddPublicKeyRequest,
    JwtRefreshRequest, JwtTokenRequest, NearSignatureMessageMetadata, Payload,
    SignatureMetadataEnum,
};
use calimero_store::Store;
use chrono::Utc;
use serde::Serialize;
use tracing::info;

use crate::admin::handlers::root_keys::store_root_key;
use crate::admin::service::{parse_api_error, AdminState, ApiError, ApiResponse};
use crate::admin::storage::client_keys::add_client_key;
use crate::admin::storage::root_key::exists_root_keys;
use crate::admin::utils::auth::{validate_challenge, validate_root_key_exists};
use crate::admin::utils::jwt::{generate_jwt_tokens, refresh_access_token};

pub fn transform_request(
    intermediate: IntermediateAddPublicKeyRequest,
) -> Result<AddPublicKeyRequest, ApiError> {
    let metadata_enum = match intermediate.wallet_metadata.wallet_type {
        WalletType::NEAR { .. } => {
            let metadata = serde_json::from_value::<NearSignatureMessageMetadata>(
                intermediate.payload.metadata,
            )
            .map_err(|_| ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid metadata.".into(),
            })?;
            SignatureMetadataEnum::NEAR(metadata)
        }
        WalletType::ETH { .. } => {
            let metadata = serde_json::from_value::<EthSignatureMessageMetadata>(
                intermediate.payload.metadata,
            )
            .map_err(|_| ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid metadata.".into(),
            })?;
            SignatureMetadataEnum::ETH(metadata)
        }
    };

    Ok(AddPublicKeyRequest {
        wallet_signature: intermediate.wallet_signature,
        payload: Payload {
            message: intermediate.payload.message,
            metadata: metadata_enum,
        },
        wallet_metadata: intermediate.wallet_metadata,
        context_id: intermediate.context_id,
    })
}

#[derive(Debug, Serialize)]
struct AddClientKeyResponse {
    data: String,
}
#[derive(Debug, Serialize)]
struct JwtResponse {
    access_token: String,
    refresh_token: String,
}

//* Register client key to authenticate client requests  */
pub async fn add_client_key_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(intermediate_req): Json<IntermediateAddPublicKeyRequest>,
) -> impl IntoResponse {
    transform_request(intermediate_req)
        // todo! experiment with Interior<Store>: WriteLayer<Interior>
        .and_then(|req| check_root_key(req, &mut state.store.clone()))
        .and_then(|req| validate_challenge(req, &state.keypair))
        // todo! experiment with Interior<Store>: WriteLayer<Interior>
        .and_then(|req| store_client_key(req, &mut state.store.clone()))
        .map_or_else(IntoResponse::into_response, |_| {
            let data: String = "Client key stored".to_owned();
            ApiResponse {
                payload: AddClientKeyResponse { data },
            }
            .into_response()
        })
}

//* Register client key to authenticate client requests  */
pub async fn generate_jwt_token_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<JwtTokenRequest>,
) -> impl IntoResponse {
    match generate_jwt_tokens(req, state.store.clone()) {
        Ok(jwt_tokens) => {
            let response = JwtResponse {
                access_token: jwt_tokens.access_token,
                refresh_token: jwt_tokens.refresh_token,
            };
            ApiResponse { payload: response }.into_response()
        }
        Err(err) => {
            eprintln!("Error generating JWT tokens: {}", err.message);
            err.into_response()
        }
    }
}

// Refresh JWT token by providing refresh token
pub async fn refresh_jwt_token_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<JwtRefreshRequest>,
) -> impl IntoResponse {
    match refresh_access_token(&req.refresh_token, state.store.clone()) {
        Ok(jwt_tokens) => {
            let response = JwtResponse {
                access_token: jwt_tokens.access_token,
                refresh_token: jwt_tokens.refresh_token,
            };
            ApiResponse { payload: response }.into_response()
        }
        Err(err) => {
            eprintln!("Error generating JWT tokens: {}", err.message);
            err.into_response()
        }
    }
}

pub fn store_client_key(
    req: AddPublicKeyRequest,
    store: &mut Store,
) -> Result<AddPublicKeyRequest, ApiError> {
    let client_key = ClientKey {
        wallet_type: req.wallet_metadata.wallet_type.clone(),
        signing_key: req.payload.message.public_key.clone(),
        created_at: Utc::now().timestamp_millis() as u64,
        context_id: req.context_id,
    };
    add_client_key(store, client_key).map_err(parse_api_error)?;
    info!("Client key stored successfully.");
    Ok(req)
}

fn check_root_key(
    req: AddPublicKeyRequest,
    store: &mut Store,
) -> Result<AddPublicKeyRequest, ApiError> {
    let root_keys = exists_root_keys(store).map_err(parse_api_error)?;
    if !root_keys {
        //first login so store root key as well
        store_root_key(
            req.wallet_metadata.signing_key.clone(),
            req.wallet_metadata.wallet_type.clone(),
            store,
        )?;
        Ok(req)
    } else {
        validate_root_key_exists(req, store)
    }
}
