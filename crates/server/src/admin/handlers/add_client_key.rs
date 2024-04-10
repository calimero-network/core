use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use calimero_identity::auth::verify_eth_signature;
use calimero_primitives::application::ApplicationId;
use calimero_store::Store;
use chrono::{Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower_sessions::Session;
use tracing::info;

use crate::admin::service::{ApiError, ApiResponse};
use crate::graphql::model::APPLICATION_ID;
use crate::storage::root_key::{get_root_key, RootKey};
use crate::verifysignature::verify_near_signature;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddClientKeyRequest {
    wallet_signature: String,
    payload: Payload,
    wallet_metadata: WalletMetadata,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Payload {
    message: SignatureMessage,
    metadata: SignatureMetadataEnum,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignatureMessage {
    nonce: String,
    application_id: String,
    timestamp: i64,
    node_signature: String,
    message: String,
    client_public_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WalletMetadata {
    #[serde(rename = "type")]
    wallet_type: WalletType,
    signing_key: String,
}

#[derive(Debug, Deserialize, PartialEq)]
enum WalletType {
    NEAR,
    ETH,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NearMetadata {
    #[serde(rename = "type")]
    wallet_type: WalletType,
    signing_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EthMetadata {
    #[serde(rename = "type")]
    wallet_type: WalletType,
    signing_key: String, // eth account 0x...
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "data")]
enum SignatureMetadataEnum {
    NEAR(NearSignatureMessageMetadata),
    ETH(EthSignatureMessageMetadata),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NearSignatureMessageMetadata {
    recipient: String,
    callback_url: String,
    nonce: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EthSignatureMessageMetadata {}

// Intermediate structs for initial parsing
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IntermediateAddClientKeyRequest {
    wallet_signature: String,
    payload: IntermediatePayload,
    wallet_metadata: WalletMetadata, // Reuse WalletMetadata as it fits the intermediate step
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IntermediatePayload {
    message: SignatureMessage, // Reuse SignatureMessage as it fits the intermediate step
    metadata: Value,           // Raw JSON value for the metadata
}

fn transform_request(
    intermediate: IntermediateAddClientKeyRequest,
) -> Result<AddClientKeyRequest, ApiError> {
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
        WalletType::ETH => {
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

    Ok(AddClientKeyRequest {
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
    _session: Session,
    State(store): State<Store>,
    Json(intermediate_req): Json<IntermediateAddClientKeyRequest>,
) -> impl IntoResponse {
    let response = transform_request(intermediate_req)
        .and_then(|req| validate_root_key_exists(req, store))
        .and_then(validate_challenge)
        .and_then(store_client_key)
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

fn verify_node_signature(
    wallet_metadata: &WalletMetadata,
    wallet_signature: &str,
    payload: &Payload,
) -> Result<bool, ApiError> {
    match wallet_metadata.wallet_type {
        WalletType::NEAR => {
            let near_metadata: &NearSignatureMessageMetadata = match &payload.metadata {
                SignatureMetadataEnum::NEAR(metadata) => metadata,
                _ => {
                    return Err(ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "Invalid metadata.".into(),
                    })
                }
            };

            let result = verify_near_signature(
                &payload.message.nonce,
                &payload.message.message,
                &near_metadata.recipient,
                &near_metadata.callback_url,
                &wallet_signature,
                &wallet_metadata.signing_key,
            );

            if !result {
                return Err(ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "Node signature is invalid. Please check the signature.".into(),
                });
            }
            Ok(true)
        }
        WalletType::ETH => {
            let _eth_metadata: &EthSignatureMessageMetadata = match &payload.metadata {
                SignatureMetadataEnum::ETH(metadata) => metadata,
                _ => {
                    return Err(ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "Invalid metadata.".into(),
                    })
                }
            };

            if let Err(err) = verify_eth_signature(
                &wallet_metadata.signing_key,
                &payload.message.message,
                wallet_signature,
            ) {
                return Err(parse_api_error(err));
            }

            Ok(true)
        }
    }
}

//Check if challenge is valid
fn validate_challenge(req: AddClientKeyRequest) -> Result<AddClientKeyRequest, ApiError> {
    validate_challenge_content(&req.payload)?;

    // Check if node has created signature
    verify_node_signature(&req.wallet_metadata, &req.wallet_signature, &req.payload)?;

    // Check challenge to verify if it has expired or not
    if is_older_than_15_minutes(req.payload.message.timestamp) {
        return Err(ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: " Challenge is too old. Please request a new challenge.".into(),
        });
    }

    Ok(req)
}

//check if signature data are not tempered with
fn validate_challenge_content(payload: &Payload) -> Result<bool, ApiError> {
    if payload.message.node_signature
        != create_node_signature(
            &payload.message.nonce,
            &payload.message.application_id,
            &payload.message.timestamp,
        )
    {
        return Err(ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: " Node signature is invalid.".into(),
        });
    }
    Ok(true)
}

fn create_node_signature(_nonce: &String, _application_id: &String, _timestamp: &i64) -> String {
    //TODO implement node signature
    // get first root key and sign the challenge

    return "abcdefhgjsdajbadk".to_string();
}

fn is_older_than_15_minutes(timestamp: i64) -> bool {
    let timestamp_datetime = Utc.timestamp_opt(timestamp, 0).unwrap();
    let now = Utc::now();
    //TODO check if timestamp is greater than now
    let duration_since_timestamp = now.signed_duration_since(timestamp_datetime);
    duration_since_timestamp > Duration::minutes(15)
}

fn validate_root_key_exists(
    req: AddClientKeyRequest,
    store: Store,
) -> Result<AddClientKeyRequest, ApiError> {
    //TODO extract from request
    let application_id = ApplicationId(APPLICATION_ID.to_string());

    //Check if root key exists
    let root_key = RootKey {
        signing_key: req.wallet_metadata.signing_key.clone(),
    };

    let existing_root_key = match get_root_key(application_id, &store, &root_key).map_err(|e| {
        info!("Error getting root key: {}", e);
        ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: e.to_string().into(),
        }
    })? {
        Some(root_key) => root_key,
        None => {
            return Err(ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Root key does not exist".into(),
            });
        }
    };

    Ok(req)
}

fn store_client_key(req: AddClientKeyRequest) -> Result<AddClientKeyRequest, ApiError> {
    //Store client public key in a list
    info!("Client key stored successfully.");

    Ok(req)
}

fn parse_api_error(err: eyre::Report) -> ApiError {
    match err.downcast::<ApiError>() {
        Ok(api_error) => api_error,
        Err(original_error) => ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: original_error.to_string(),
        },
    }
}
