use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use calimero_identity::auth::verify_eth_signature;
use calimero_primitives::identity::{NearNetworkId, WalletType};
use calimero_server_primitives::admin::{
    AddPublicKeyRequest, EthSignatureMessageMetadata, NearSignatureMessageMetadata,
    NodeChallengeMessage, Payload, SignatureMessage, SignatureMetadataEnum,
    StarknetSignatureMessageMetadata, WalletMetadata, WalletSignature,
};
use calimero_store::Store;
use chrono::{Duration, TimeZone, Utc};
use libp2p::identity::Keypair;
use reqwest::StatusCode;
use serde_json::to_string as to_json_string;
use tracing::info;

use crate::admin::handlers::root_keys::store_root_key;
use crate::admin::service::{parse_api_error, ApiError};
use crate::admin::storage::root_key::{get_root_key, has_near_account_root_key};
use crate::verifywalletsignatures::near::{check_for_near_account_key, verify_near_signature};
use crate::verifywalletsignatures::starknet::{verify_argent_signature, verify_metamask_signature};

// TODO: Consider breaking this function up into pieces.
#[allow(clippy::too_many_lines)]
pub async fn verify_node_signature(
    wallet_metadata: &WalletMetadata,
    wallet_signature: &WalletSignature,
    payload: &Payload,
) -> Result<bool, ApiError> {
    match wallet_metadata.wallet_type {
        WalletType::NEAR { .. } => {
            #[allow(clippy::wildcard_enum_match_arm)]
            let near_metadata: &NearSignatureMessageMetadata = match &payload.metadata {
                SignatureMetadataEnum::NEAR(metadata) => metadata,
                SignatureMetadataEnum::ETH(_) => {
                    return Err(ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "Invalid metadata.".into(),
                    })
                }
                _ => {
                    return Err(ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "Unsupported metadata.".into(),
                    })
                }
            };

            let WalletSignature::String(signature_str) = wallet_signature else {
                return Err(ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "Invalid wallet signature type.".into(),
                });
            };

            let result = verify_near_signature(
                &payload.message.nonce,
                &payload.message.message,
                &near_metadata.recipient,
                &near_metadata.callback_url,
                signature_str,
                &wallet_metadata.verifying_key,
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
            #[allow(clippy::wildcard_enum_match_arm)]
            let _eth_metadata: &EthSignatureMessageMetadata = match &payload.metadata {
                SignatureMetadataEnum::ETH(metadata) => metadata,
                SignatureMetadataEnum::NEAR(_) => {
                    return Err(ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "Invalid metadata.".into(),
                    })
                }
                _ => {
                    return Err(ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "Unsupported metadata.".into(),
                    })
                }
            };

            let WalletSignature::String(signature_str) = wallet_signature else {
                return Err(ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "Invalid wallet signature type.".into(),
                });
            };

            if let Err(err) = verify_eth_signature(
                &wallet_metadata.verifying_key,
                &payload.message.message,
                signature_str,
            ) {
                return Err(parse_api_error(err));
            }

            Ok(true)
        }
        WalletType::STARKNET { ref wallet_name } => {
            #[allow(clippy::wildcard_enum_match_arm)]
            let _sn_metadata: &StarknetSignatureMessageMetadata = match &payload.metadata {
                SignatureMetadataEnum::STARKNET(metadata) => metadata,
                _ => {
                    return Err(ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "Invalid metadata.".into(),
                    })
                }
            };

            #[allow(clippy::wildcard_enum_match_arm)]
            let (message_hash, signature) = match wallet_signature {
                WalletSignature::StarknetPayload(payload) => {
                    (&payload.message_hash, &payload.signature)
                }
                _ => {
                    return Err(ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "Invalid wallet signature type for Starknet.".into(),
                    })
                }
            };

            let Some(network_metadata) = &wallet_metadata.network_metadata else {
                return Err(ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "Missing network_metadata for Starknet.".into(),
                });
            };

            // Now extract `rpc_url` and `chain_id` from the `network_metadata`
            let rpc_node_url = network_metadata.rpc_url.clone();
            let chain_id = network_metadata.chain_id.clone();

            let result = match wallet_name.as_str() {
                "argentX" => {
                    verify_argent_signature(
                        message_hash,
                        signature.clone(),
                        &wallet_metadata.verifying_key,
                        &payload.message.message,
                        &rpc_node_url,
                        &chain_id,
                    )
                    .await
                }
                "metamask" => {
                    let Some(wallet_address) = &wallet_metadata.wallet_address else {
                        return Err(ApiError {
                            status_code: StatusCode::BAD_REQUEST,
                            message: "Wallet address not present.".into(),
                        });
                    };
                    verify_metamask_signature(
                        message_hash,
                        &signature,
                        &wallet_metadata.verifying_key,
                        &payload.message.message,
                        wallet_address,
                        &chain_id,
                    )
                }
                _ => {
                    return Err(ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "Invalid wallet name for Starknet.".into(),
                    })
                }
            };

            if let Err(err) = result {
                return Err(parse_api_error(err));
            }

            Ok(true)
        }
        _ => Err(ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Unsupported wallet type.".into(),
        }),
    }
}

//Check if challenge is valid
pub async fn validate_challenge(
    req: AddPublicKeyRequest,
    keypair: &Keypair,
) -> Result<AddPublicKeyRequest, ApiError> {
    validate_challenge_content(&req.payload, keypair)?;

    // Check if node has created signature
    let _ =
        verify_node_signature(&req.wallet_metadata, &req.wallet_signature, &req.payload).await?;

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
    Ok(NodeChallengeMessage::new(
        message.nonce.clone(),
        message.context_id,
        message.timestamp,
    ))
}

pub fn decode_signature(encoded_sig: &String) -> Result<Vec<u8>, ApiError> {
    STANDARD.decode(encoded_sig).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Failed to decode signature".into(),
    })
}

pub fn serialize_node_challenge(challenge: &NodeChallengeMessage) -> Result<String, ApiError> {
    to_json_string(challenge).map_err(|_| ApiError {
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

pub async fn validate_root_key_exists(
    req: AddPublicKeyRequest,
    store: &Store,
) -> Result<AddPublicKeyRequest, ApiError> {
    let root_key_result = get_root_key(store, &req.wallet_metadata.verifying_key).map_err(|e| {
        info!("Error getting root key: {}", e);
        ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: e.to_string(),
        }
    })?;

    drop(match root_key_result {
        Some(root_key) => root_key,
        None => {
            if let WalletType::NEAR { network_id } = &req.wallet_metadata.wallet_type {
                let wallet_address = match req.wallet_metadata.wallet_address.as_deref() {
                    Some(address) => address,
                    None => {
                        return Err(ApiError {
                            status_code: StatusCode::BAD_REQUEST,
                            message: "Wallet address not present".to_string(),
                        });
                    }
                };
                // Check if the wallet_address has a NEAR account key from DB
                let near_keys: String = match has_near_account_root_key(store, wallet_address) {
                    Ok(keys) if keys.is_empty() => {
                        return Err(ApiError {
                            status_code: StatusCode::BAD_REQUEST,
                            message: "Root key does not exist".into(),
                        });
                    }
                    Ok(keys) => keys,
                    Err(err) => {
                        info!("Error checking if near client key exists: {}", err);
                        return Err(ApiError {
                            status_code: StatusCode::INTERNAL_SERVER_ERROR,
                            message: err.to_string(),
                        });
                    }
                };

                // Extract wallet_address as a &str
                let wallet_address = match req.wallet_metadata.wallet_address.as_deref() {
                    Some(address) => address,
                    None => {
                        return Err(ApiError {
                            status_code: StatusCode::BAD_REQUEST,
                            message: "Wallet address not present".to_string(),
                        });
                    }
                };

                // Get network_type and from it use correct rpc_url
                let rpc_url = match network_id {
                    NearNetworkId::Mainnet => "https://rpc.mainnet.near.org",
                    NearNetworkId::Testnet => "https://rpc.testnet.near.org",
                    _ => {
                        // Handle the case where the network ID is unknown or not handled
                        return Err(ApiError {
                            status_code: StatusCode::BAD_REQUEST,
                            message: "Unknown NEAR network ID".into(),
                        });
                    }
                };

                // Check if the given public key is from the given NEAR account
                if !check_for_near_account_key(
                    &req.wallet_metadata.verifying_key,
                    wallet_address,
                    rpc_url,
                )
                .await?
                {
                    return Err(ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "Given public key is not from wallet".into(),
                    });
                }
                // Check if the wallet_address has a NEAR account key from DB
                match check_for_near_account_key(&near_keys, wallet_address, rpc_url).await? {
                    true => {
                        let _ = store_root_key(
                            req.wallet_metadata.verifying_key.clone(),
                            req.wallet_metadata.wallet_type.clone(),
                            wallet_address.to_string(),
                            store,
                        )
                        .map_err(|err| {
                            return err;
                        })?;
                        return Ok(req);
                    }
                    false => {
                        return Err(ApiError {
                            status_code: StatusCode::BAD_REQUEST,
                            message: "Root key does not exist for given wallet".into(),
                        });
                    }
                }
            }
            return Err(ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Root key does not exist".into(),
            });
        }
    });

    Ok(req)
}
