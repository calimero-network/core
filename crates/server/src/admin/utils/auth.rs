use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use calimero_primitives::identity::{NearNetworkId, WalletType};
use calimero_server_primitives::admin::{
    AddPublicKeyRequest, EthSignatureMessageMetadata, NearSignatureMessageMetadata,
    NodeChallengeMessage, Payload, SignatureMessage, SignatureMetadataEnum,
    StarknetSignatureMessageMetadata, WalletMetadata, WalletSignature,
};
use calimero_store::Store;
use chrono::{Duration, TimeZone, Utc};
use eyre::{bail, eyre, Result as EyreResult};
use libp2p::identity::Keypair;
use libp2p::identity::PublicKey;
use reqwest::StatusCode;
use serde_json::to_string as to_json_string;
use tracing::info;
use web3::signing::{keccak256, recover};

use crate::admin::handlers::root_keys::store_root_key;
use crate::admin::service::{parse_api_error, ApiError};
use crate::admin::storage::root_key::{get_root_key, has_near_account_root_key};
use crate::verifywalletsignatures::near::{has_near_key, verify_near_signature};
use crate::verifywalletsignatures::starknet::{verify_argent_signature, verify_metamask_signature};

// TODO: Consider breaking this function up into pieces.
/// Verifies a node signature based on the type of wallet (NEAR, ETH, STARKNET).
///
/// # Arguments
/// * `wallet_metadata` - Contains metadata about the wallet, including wallet type.
/// * `wallet_signature` - Signature from the wallet for verification.
/// * `payload` - Data that is signed by the wallet.
///
/// # Returns
/// * `Ok(true)` - If the signature is valid.
/// * `Err(ApiError)` - If the signature is invalid or the wallet type is unsupported.
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
                SignatureMetadataEnum::ETH(metadata) => Ok(metadata), // Return Ok for the valid case
                SignatureMetadataEnum::NEAR(_) => Err(ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "Invalid metadata.".into(),
                }),
                _ => Err(ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "Unsupported metadata.".into(),
                }),
            }?;

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

/// Validates the challenge by verifying the node signature and checking if it has expired.
///
/// # Arguments
/// * `req` - Request containing public key and signature data.
/// * `keypair` - The node's keypair for signature verification.
///
/// # Returns
/// * `Ok(AddPublicKeyRequest)` - If the challenge is valid.
/// * `Err(ApiError)` - If the challenge is invalid or expired.
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

/// Validates the content of the challenge by checking the node's signature.
///
/// # Arguments
/// * `payload` - The signed payload containing the challenge.
/// * `keypair` - The node's keypair used for signing.
///
/// # Returns
/// * `Ok(())` - If the signature is valid.
/// * `Err(ApiError)` - If the signature is invalid.
pub fn validate_challenge_content(payload: &Payload, keypair: &Keypair) -> Result<(), ApiError> {
    let node_challenge = construct_node_challenge(&payload.message)?;
    let signature = decode_signature(&payload.message.node_signature)?;
    let message = serialize_node_challenge(&node_challenge)?;

    verify_signature(&message, &signature, keypair)
}

/// Constructs a node challenge message from the provided signature message.
///
/// # Arguments
/// * `message` - The signature message containing challenge details.
///
/// # Returns
/// * `Ok(NodeChallengeMessage)` - The constructed challenge.
/// * `Err(ApiError)` - If there is an error constructing the challenge.
pub fn construct_node_challenge(
    message: &SignatureMessage,
) -> Result<NodeChallengeMessage, ApiError> {
    Ok(NodeChallengeMessage::new(
        message.nonce.clone(),
        message.context_id,
        message.timestamp,
    ))
}

/// Decodes a base64-encoded signature.
///
/// # Arguments
/// * `encoded_sig` - The encoded signature string.
///
/// # Returns
/// * `Ok(Vec<u8>)` - The decoded signature.
/// * `Err(ApiError)` - If decoding fails.
pub fn decode_signature(encoded_sig: &String) -> Result<Vec<u8>, ApiError> {
    STANDARD.decode(encoded_sig).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: "Failed to decode signature".into(),
    })
}

/// Serializes the node challenge into a JSON string.
///
/// # Arguments
/// * `challenge` - The challenge message to be serialized.
///
/// # Returns
/// * `Ok(String)` - The serialized challenge.
/// * `Err(ApiError)` - If serialization fails.
pub fn serialize_node_challenge(challenge: &NodeChallengeMessage) -> Result<String, ApiError> {
    to_json_string(challenge).map_err(|_| ApiError {
        status_code: StatusCode::INTERNAL_SERVER_ERROR,
        message: "Failed to deserialize challenge data".into(),
    })
}

/// Verifies the signature of a message using the node's keypair.
///
/// # Arguments
/// * `message` - The message to verify.
/// * `signature` - The signature to check against.
/// * `keypair` - The node's keypair used for signing.
///
/// # Returns
/// * `Ok(())` - If the signature is valid.
/// * `Err(ApiError)` - If the signature is invalid.
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

/// Checks if a given timestamp is older than 15 minutes from the current time.
///
/// # Arguments
/// * `timestamp` - The timestamp to check.
///
/// # Returns
/// * `true` - If the timestamp is older than 15 minutes.
/// * `false` - Otherwise.
#[must_use]
pub fn is_older_than_15_minutes(timestamp: i64) -> bool {
    let timestamp_datetime = Utc.timestamp_opt(timestamp, 0).unwrap();
    let now = Utc::now();
    //TODO check if timestamp is greater than now
    let duration_since_timestamp = now.signed_duration_since(timestamp_datetime);
    duration_since_timestamp > Duration::minutes(15)
}

/// Validates if the root key exists for the given request.
///
/// # Arguments
/// * `req` - The request containing the wallet metadata.
/// * `store` - The store to look up the root key.
///
/// # Returns
/// * `Ok(AddPublicKeyRequest)` - If the root key exists.
/// * `Err(ApiError)` - If the root key does not exist or other errors occur.
pub async fn validate_root_key_exists(
    req: AddPublicKeyRequest,
    store: &Store,
) -> Result<AddPublicKeyRequest, ApiError> {
    if get_root_key(store, &req.wallet_metadata.verifying_key)
        .map_err(|e| {
            info!("Error getting root key: {}", e);
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: e.to_string(),
            }
        })?
        .is_none()
    {
        if let WalletType::NEAR { network_id } = &req.wallet_metadata.wallet_type {
            let wallet_address = req
                .wallet_metadata
                .wallet_address
                .as_deref()
                .ok_or(ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "Wallet address not present".to_string(),
                })?;

            let near_keys: String = has_near_account_root_key(store, wallet_address)
                .map(|keys| {
                    if keys.is_empty() {
                        Err(ApiError {
                            status_code: StatusCode::BAD_REQUEST,
                            message: "Root key does not exist".into(),
                        })
                    } else {
                        Ok(keys)
                    }
                })
                .map_err(|err| {
                    info!("Error checking if near client key exists: {}", err);
                    ApiError {
                        status_code: StatusCode::INTERNAL_SERVER_ERROR,
                        message: err.to_string(),
                    }
                })??;

            let rpc_url = match network_id {
                NearNetworkId::Mainnet => Ok("https://rpc.mainnet.near.org"),
                NearNetworkId::Testnet => Ok("https://rpc.testnet.near.org"),
                _ => Err(ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "Unknown NEAR network ID".into(),
                }),
            }?;

            if !has_near_key(&req.wallet_metadata.verifying_key, wallet_address, rpc_url).await? {
                return Err(ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: format!(
                        "Provided public key does not belong to account {:?}",
                        wallet_address
                    ),
                });
            }

            if has_near_key(&near_keys, wallet_address, rpc_url).await? {
                let _ = store_root_key(
                    req.wallet_metadata.verifying_key.clone(),
                    req.wallet_metadata.wallet_type.clone(),
                    wallet_address.to_string(),
                    store,
                )
                .map_err(|err| err)?;
            } else {
                return Err(ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "Root key does not exist for given wallet".into(),
                });
            }

            return Ok(req);
        }

        return Err(ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Root key does not exist".into(),
        });
    }
    Ok(req)
}

pub fn verify_near_public_key(public_key: &str, msg: &[u8], signature: &[u8]) -> EyreResult<bool> {
    let public_key = bs58::decode(public_key)
        .into_vec()
        .map_err(|_| eyre!("Invalid public key: Base58 encoding error"))?;

    let public_key = PublicKey::try_decode_protobuf(&public_key)
        .map_err(|_| eyre!("Invalid public key: Protobuf encoding error"))?;

    Ok(public_key.verify(msg, signature))
}

pub fn verify_eth_signature(account: &str, message: &str, signature: &str) -> EyreResult<bool> {
    let Ok(signature_bytes) = hex::decode(signature.trim_start_matches("0x")) else {
        bail!("Cannot decode signature.")
    };

    // Ensure the signature is the correct length (65 bytes)
    if signature_bytes.len() != 65 {
        bail!("Signature must be 65 bytes long.")
    }

    let message_hash = eth_message(message);
    let recovery_id = i32::from(signature_bytes[64]).saturating_sub(27_i32);

    // Attempt to recover the public key, returning false if recovery fails
    match recover(&message_hash, &signature_bytes[..64], recovery_id) {
        Ok(pubkey) => {
            // Format the recovered public key as a hex string
            let pubkey_hex = format!("{pubkey:02X?}");
            // Compare the recovered public key with the account address in a case-insensitive manner
            let result = account.eq_ignore_ascii_case(&pubkey_hex);
            if !result {
                bail!("Public key and account does not match.")
            }
            Ok(true)
        }
        Err(_) => bail!("Cannot recover public key."),
    }
}

pub fn eth_message(message: &str) -> [u8; 32] {
    keccak256(
        format!(
            "{}{}{}",
            "\x19Ethereum Signed Message:\n",
            message.len(),
            message
        )
        .as_bytes(),
    )
}
