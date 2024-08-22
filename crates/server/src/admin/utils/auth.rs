use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use calimero_identity::auth::verify_eth_signature;
use calimero_primitives::identity::WalletType;
use calimero_server_primitives::admin::{
    AddPublicKeyRequest, EthSignatureMessageMetadata, NearSignatureMessageMetadata,
    NodeChallengeMessage, Payload, SignatureMessage, SignatureMetadataEnum, WalletMetadata,
};
use calimero_store::Store;
use chrono::{Duration, TimeZone, Utc};
use libp2p::identity::Keypair;
use reqwest::StatusCode;
use tracing::info;

use crate::admin::service::{parse_api_error, ApiError};
use crate::admin::storage::root_key::get_root_key;
use crate::verifysignature::verify_near_signature;

pub fn verify_node_signature(
    wallet_metadata: &WalletMetadata,
    wallet_signature: &str,
    payload: &Payload,
) -> Result<bool, ApiError> {
    match wallet_metadata.wallet_type {
        WalletType::NEAR { .. } => {
            let near_metadata: &NearSignatureMessageMetadata = match &payload.metadata {
                SignatureMetadataEnum::NEAR(metadata) => metadata,
                SignatureMetadataEnum::ETH(_) => {
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
                wallet_signature,
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
        WalletType::ETH { .. } => {
            let _eth_metadata: &EthSignatureMessageMetadata = match &payload.metadata {
                SignatureMetadataEnum::ETH(metadata) => metadata,
                SignatureMetadataEnum::NEAR(_) => {
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
pub fn validate_challenge(
    req: AddPublicKeyRequest,
    keypair: &Keypair,
) -> Result<AddPublicKeyRequest, ApiError> {
    validate_challenge_content(&req.payload, keypair)?;

    // Check if node has created signature
    let _ = verify_node_signature(&req.wallet_metadata, &req.wallet_signature, &req.payload)?;

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
pub fn validate_challenge_content(payload: &Payload, keypair: &Keypair) -> Result<(), ApiError> {
    let node_challenge = construct_node_challenge(&payload.message)?;
    let signature = decode_signature(&payload.message.node_signature)?;
    let message = serialize_node_challenge(&node_challenge)?;

    verify_signature(&message, &signature, keypair)
}

pub fn construct_node_challenge(
    message: &SignatureMessage,
) -> Result<NodeChallengeMessage, ApiError> {
    Ok(NodeChallengeMessage {
        nonce: message.nonce.clone(),
        context_id: message.context_id,
        timestamp: message.timestamp,
    })
}

pub fn decode_signature(encoded_sig: &String) -> Result<Vec<u8>, ApiError> {
    STANDARD.decode(encoded_sig).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Failed to decode signature".into(),
    })
}

pub fn serialize_node_challenge(challenge: &NodeChallengeMessage) -> Result<String, ApiError> {
    serde_json::to_string(challenge).map_err(|_| ApiError {
        status_code: StatusCode::INTERNAL_SERVER_ERROR,
        message: "Failed to deserialize challenge data".into(),
    })
}

pub fn verify_signature(
    message: &String,
    signature: &[u8],
    keypair: &Keypair,
) -> Result<(), ApiError> {
    if keypair.public().verify(message.as_bytes(), signature) {
        Ok(())
    } else {
        Err(ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Node signature is invalid.".into(),
        })
    }
}

#[must_use]
pub fn is_older_than_15_minutes(timestamp: i64) -> bool {
    let timestamp_datetime = Utc.timestamp_opt(timestamp, 0).unwrap();
    let now = Utc::now();
    //TODO check if timestamp is greater than now
    let duration_since_timestamp = now.signed_duration_since(timestamp_datetime);
    duration_since_timestamp > Duration::minutes(15)
}

pub fn validate_root_key_exists(
    req: AddPublicKeyRequest,
    store: &mut Store,
) -> Result<AddPublicKeyRequest, ApiError> {
    let root_key_result = get_root_key(store, &req.wallet_metadata.signing_key).map_err(|e| {
        info!("Error getting root key: {}", e);
        ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: e.to_string(),
        }
    })?;

    drop(match root_key_result {
        Some(root_key) => root_key,
        None => {
            return Err(ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Root key does not exist".into(),
            });
        }
    });

    Ok(req)
}
