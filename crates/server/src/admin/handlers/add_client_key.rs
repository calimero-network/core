use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::identity::{ClientKey, WalletType};
use calimero_server_primitives::admin::{
    AddPublicKeyRequest, EthSignatureMessageMetadata, IntermediateAddPublicKeyRequest,
    JwtRefreshRequest, JwtTokenRequest, NearSignatureMessageMetadata, Payload,
    SignatureMetadataEnum, StarknetSignatureMessageMetadata,
};
use calimero_store::Store;
use chrono::Utc;
use futures_util::TryFutureExt;
use serde::Serialize;
use serde_json::from_value as from_json_value;
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
            let metadata =
                from_json_value::<NearSignatureMessageMetadata>(intermediate.payload.metadata)
                    .map_err(|_| ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "Invalid metadata.".into(),
                    })?;
            SignatureMetadataEnum::NEAR(metadata)
        }
        WalletType::ETH { .. } => {
            let metadata =
                from_json_value::<EthSignatureMessageMetadata>(intermediate.payload.metadata)
                    .map_err(|_| ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "Invalid metadata.".into(),
                    })?;
            SignatureMetadataEnum::ETH(metadata)
        }
        WalletType::STARKNET { .. } => {
            let metadata =
                from_json_value::<StarknetSignatureMessageMetadata>(intermediate.payload.metadata)
                    .map_err(|_| ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "Invalid metadata.".into(),
                    })?;
            SignatureMetadataEnum::STARKNET(metadata)
        }
        _ => {
            return Err(ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Unsupported wallet type.".into(),
            });
        }
    };

    Ok(AddPublicKeyRequest::new(
        intermediate.wallet_signature,
        Payload::new(intermediate.payload.message, metadata_enum),
        intermediate.wallet_metadata,
        intermediate.context_id,
    ))
}

#[derive(Debug, Serialize)]
struct AddClientKeyResponse {
    data: String,
}
#[derive(Debug, Serialize)]
struct JwtResponse {
    data: JwtTokens,
}

#[derive(Debug, Serialize)]
struct JwtTokens {
    access_token: String,
    refresh_token: String,
}

//* Register client key to authenticate client requests  */
pub async fn add_client_key_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(intermediate_req): Json<IntermediateAddPublicKeyRequest>,
) -> impl IntoResponse {
    async { transform_request(intermediate_req) }
        // todo! experiment with Interior<Store>: WriteLayer<Interior>
        .and_then(|req| check_root_key(req, &state.store))
        .and_then(|req| validate_challenge(req, &state.keypair))
        // todo! experiment with Interior<Store>: WriteLayer<Interior>
        .and_then(|req| async { store_client_key(req, &state.store) })
        .await
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
            let tokens = JwtTokens {
                access_token: jwt_tokens.access_token,
                refresh_token: jwt_tokens.refresh_token,
            };
            let response = JwtResponse { data: tokens };
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
            let tokens = JwtTokens {
                access_token: jwt_tokens.access_token,
                refresh_token: jwt_tokens.refresh_token,
            };
            let response = JwtResponse { data: tokens };
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
    store: &Store,
) -> Result<AddPublicKeyRequest, ApiError> {
    #[allow(clippy::cast_sign_loss)]
    let client_key = ClientKey::new(
        req.wallet_metadata.wallet_type.clone(),
        req.payload.message.public_key.clone(),
        Utc::now().timestamp_millis() as u64,
        req.context_id,
    );
    let _ = add_client_key(store, client_key).map_err(parse_api_error)?;
    info!("Client key stored successfully.");
    Ok(req)
}

async fn check_root_key(
    req: AddPublicKeyRequest,
    store: &Store,
) -> Result<AddPublicKeyRequest, ApiError> {
    let root_keys = exists_root_keys(store).map_err(parse_api_error)?;
    if root_keys {
        // If root keys exist, validate and return the request
        validate_root_key_exists(req, store).await
    } else {
        // Attempt to store the root key, then return the request
        let wallet_address = match req.wallet_metadata.wallet_type {
            WalletType::NEAR { .. } => req
                .wallet_metadata
                .wallet_address
                .clone()
                .unwrap_or(String::new()),
            _ => String::new(), // Handle other cases appropriately
        };

        let _ = store_root_key(
            req.wallet_metadata.verifying_key.clone(),
            req.wallet_metadata.wallet_type.clone(),
            wallet_address,
            store,
        )
        .map_err(|err| {
            return err;
        })?;

        Ok(req)
    }
}
