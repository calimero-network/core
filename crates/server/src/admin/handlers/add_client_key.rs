use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use calimero_identity::auth::verify_eth_signature;
use calimero_store::Store;
use chrono::{Duration, TimeZone, Utc};
use libp2p::identity::Keypair;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::info;

use crate::admin::service::{AdminState, ApiError, ApiResponse, NodeChallengeMessage};
use crate::admin::storage::client_keys::{add_client_key, ClientKey};
use crate::admin::storage::root_key::{get_root_key, RootKey};
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

#[derive(Debug, Deserialize, PartialEq, Serialize, Clone)]
pub enum WalletType {
    NEAR,
    ETH,
}

impl WalletType {
    pub fn from_str(input: &str) -> Result<Self, eyre::Report> {
        match input {
            "ETH" => Ok(WalletType::ETH),
            "NEAR" => Ok(WalletType::NEAR),
            _ => eyre::bail!("Invalid wallet_type value"),
        }
    }
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
    Extension(state): Extension<Arc<AdminState>>,
    Json(intermediate_req): Json<IntermediateAddClientKeyRequest>,
) -> impl IntoResponse {
    let response = transform_request(intermediate_req)
        .and_then(|req| validate_root_key_exists(req, &state.store))
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

fn store_client_key(
    req: AddClientKeyRequest,
    store: &Store,
) -> Result<AddClientKeyRequest, ApiError> {
    let client_key = ClientKey {
        wallet_type: WalletType::NEAR,
        signing_key: req.payload.message.client_public_key.clone(),
    };
    add_client_key(&store, client_key).map_err(|e| parse_api_error(e))?;
    info!("Client key stored successfully.");
    Ok(req)
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
fn validate_challenge(
    req: AddClientKeyRequest,
    keypair: &Keypair,
) -> Result<AddClientKeyRequest, ApiError> {
    validate_challenge_content(&req.payload, keypair)?;

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
fn validate_challenge_content(payload: &Payload, keypair: &Keypair) -> Result<(), ApiError> {
    let node_challenge = construct_node_challenge(&payload.message)?;
    let signature = decode_signature(&payload.message.node_signature)?;
    let message = serialize_node_challenge(&node_challenge)?;

    verify_signature(&message, &signature, keypair)
}

fn construct_node_challenge(message: &SignatureMessage) -> Result<NodeChallengeMessage, ApiError> {
    Ok(NodeChallengeMessage {
        nonce: message.nonce.clone(),
        application_id: message.application_id.clone(),
        timestamp: message.timestamp,
    })
}

fn decode_signature(encoded_sig: &String) -> Result<Vec<u8>, ApiError> {
    STANDARD.decode(encoded_sig).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Failed to decode signature".into(),
    })
}

fn serialize_node_challenge(challenge: &NodeChallengeMessage) -> Result<String, ApiError> {
    serde_json::to_string(challenge).map_err(|_| ApiError {
        status_code: StatusCode::INTERNAL_SERVER_ERROR,
        message: "Failed to deserialize challenge data".into(),
    })
}

fn verify_signature(message: &String, signature: &[u8], keypair: &Keypair) -> Result<(), ApiError> {
    if keypair.public().verify(message.as_bytes(), signature) {
        Ok(())
    } else {
        Err(ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Node signature is invalid.".into(),
        })
    }
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
    store: &Store,
) -> Result<AddClientKeyRequest, ApiError> {
    //Check if root key exists
    let root_key = RootKey {
        signing_key: req.wallet_metadata.signing_key.clone(),
    };

    match get_root_key(&store, &root_key).map_err(|e| {
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

pub fn parse_api_error(err: eyre::Report) -> ApiError {
    match err.downcast::<ApiError>() {
        Ok(api_error) => api_error,
        Err(original_error) => ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: original_error.to_string(),
        },
    }
}
