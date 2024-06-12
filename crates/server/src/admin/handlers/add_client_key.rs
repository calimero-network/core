use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::identity::{ClientKey, WalletType};
use calimero_server_primitives::admin::{
    AddPublicKeyRequest, EthSignatureMessageMetadata, IntermediateAddPublicKeyRequest,
    NearSignatureMessageMetadata, Payload, SignatureMetadataEnum,
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

pub fn transform_request(
    intermediate: IntermediateAddPublicKeyRequest,
) -> Result<AddPublicKeyRequest, ApiError> {
    let metadata_enum = match intermediate.wallet_metadata.wallet_type {
        WalletType::NEAR => {
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
    })
}

#[derive(Debug, Serialize)]
struct AddClientKeyResponse {
    data: String,
}

//* Register client key to authenticate client requests  */
pub async fn add_client_key_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(intermediate_req): Json<IntermediateAddPublicKeyRequest>,
) -> impl IntoResponse {
    let response = transform_request(intermediate_req)
        .and_then(|req| check_root_key(req, &state.store))
        .and_then(|req| validate_challenge(req, &state.keypair))
        .and_then(|req| store_client_key(req, &state.store))
        .map_or_else(
            |err| err.into_response(),
            |_| {
                let data: String = "Client key stored".to_string();
                ApiResponse {
                    payload: AddClientKeyResponse { data },
                }
                .into_response()
            },
        );

    response
}

pub fn store_client_key(
    req: AddPublicKeyRequest,
    store: &Store,
) -> Result<AddPublicKeyRequest, ApiError> {
    let client_key = ClientKey {
        wallet_type: WalletType::NEAR,
        signing_key: req.payload.message.public_key.clone(),
        created_at: Utc::now().timestamp_millis() as u64,
        application_id: req.payload.message.application_id.clone()
    };
    add_client_key(&store, client_key).map_err(|e| parse_api_error(e))?;
    info!("Client key stored successfully.");
    Ok(req)
}

fn check_root_key(
    req: AddPublicKeyRequest,
    store: &Store,
) -> Result<AddPublicKeyRequest, ApiError> {
    let root_keys = exists_root_keys(&store).map_err(|e| parse_api_error(e))?;
    if !root_keys {
        //first login so store root key as well
        store_root_key(
            req.wallet_metadata.signing_key.clone(),
            req.wallet_metadata.wallet_type,
            &store,
        )?;
        Ok(req)
    } else {
        validate_root_key_exists(req, &store)
    }
}
