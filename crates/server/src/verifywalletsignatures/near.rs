#[cfg(test)]
#[path = "../tests/verifywalletsignatures/near.rs"]
mod tests;

use base64::engine::general_purpose::STANDARD;
use base64::engine::Engine;
use borsh::BorshSerialize;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use eyre::{eyre, Report, Result as EyreResult};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::info;

use crate::admin::service::ApiError;

/// A generic structure for NEAR JSON-RPC responses.
///
/// # Fields
/// * `jsonrpc` - The version of the JSON-RPC protocol.
/// * `result` - The result data if the query was successful.
/// * `error` - The error data if the query failed.
/// * `id` - The request ID.
#[derive(Debug, Serialize, Deserialize)]
struct NearJsonRpcResponse<T> {
    jsonrpc: String,
    result: Option<T>,
    error: Option<NearJsonRpcError>, // Top-level error
    id: String,
}

/// Represents an error from a NEAR JSON-RPC query.
///
/// # Fields
/// * `code` - The error code.
/// * `message` - The error message.
/// * `data` - Optional additional error data.
#[derive(Debug, Serialize, Deserialize)]
struct NearJsonRpcError {
    code: i64,
    message: String,
    data: Option<String>,
}

/// Represents the result data from a NEAR JSON-RPC query, including permissions.
///
/// # Fields
/// * `block_hash` - The hash of the block containing the result.
/// * `block_height` - The height of the block.
/// * `nonce` - An optional nonce.
/// * `permission` - The permission level granted to the key.
/// * `error` - An optional error string at the result level.
#[derive(Debug, Serialize, Deserialize)]
struct ResultDataWithPermission {
    block_hash: String,
    block_height: u64,
    nonce: Option<u64>,
    permission: Option<Permission>,
    error: Option<String>, // Result-level error
}

/// Represents the permission level of a NEAR key, which can be a function call or full access.
///
/// # Variants
/// * `FunctionCall` - Grants function call access.
/// * `FullAccess` - Grants full access to the account.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum Permission {
    FunctionCall(FunctionCall),
    FullAccess(String),
}

/// Represents a function call permission, including the allowed methods and receiver ID.
///
/// # Fields
/// * `allowance` - The allowance for function calls.
/// * `receiver_id` - The receiver account ID.
/// * `method_names` - The methods that can be called.
#[derive(Debug, Serialize, Deserialize)]
struct FunctionCall {
    allowance: String,
    receiver_id: String,
    method_names: Vec<String>,
}

/// Represents an RPC request sent to the NEAR JSON-RPC API.
///
/// # Fields
/// * `jsonrpc` - The version of the JSON-RPC protocol.
/// * `id` - The request ID.
/// * `method` - The method to be called.
/// * `params` - The parameters for the method.
#[derive(Serialize, Deserialize, Debug)]
struct RpcRequest {
    jsonrpc: String,
    id: String,
    method: String,
    params: RpcParams,
}

/// Represents the parameters for an RPC request.
///
/// # Fields
/// * `request_type` - The type of request (e.g., "view_access_key").
/// * `finality` - The finality level of the query (e.g., "final").
/// * `account_id` - The account ID to query.
/// * `public_key` - The public key to query.
#[derive(Serialize, Deserialize, Debug)]
struct RpcParams {
    request_type: String,
    finality: String,
    account_id: String,
    public_key: String,
}

/// Creates a `Payload` struct from the provided message, nonce, recipient, and callback URL.
///
/// # Arguments
/// * `message` - The message to include in the payload.
/// * `nonce` - A 32-byte nonce.
/// * `recipient` - The recipient of the message.
/// * `callback_url` - The callback URL for the message.
///
/// # Returns
/// * `Payload` - The constructed payload.
fn create_payload(message: &str, nonce: [u8; 32], recipient: &str, callback_url: &str) -> Payload {
    Payload {
        tag: 2_147_484_061,
        message: message.to_owned(),
        nonce,
        recipient: recipient.to_owned(),
        callback_url: Some(callback_url.to_owned()),
    }
}

/// Hashes the given bytes using SHA-256.
///
/// # Arguments
/// * `bytes` - The bytes to hash.
///
/// # Returns
/// * `[u8; 32]` - The SHA-256 hash of the bytes.
fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();

    hasher.update(bytes);

    let result = hasher.finalize();

    let mut hash_array = [0_u8; 32];
    hash_array.copy_from_slice(&result);

    hash_array
}

/// Verifies a NEAR signature by checking the payload, nonce, and signature.
///
/// # Arguments
/// * `challenge` - A base64-encoded challenge string (nonce).
/// * `message` - The message that was signed.
/// * `app` - The recipient app.
/// * `callback_url` - The callback URL.
/// * `signature_base64` - The base64-encoded signature.
/// * `public_key_str` - The public key string used to verify the signature.
///
/// # Returns
/// * `true` - If the signature is valid.
/// * `false` - If the signature is invalid or decoding fails.
pub fn verify_near_signature(
    challenge: &str,
    message: &str,
    app: &str,
    callback_url: &str,
    signature_base64: &str,
    public_key_str: &str,
) -> bool {
    let Ok(nonce) = decode_to_fixed_array::<32>(&Encoding::Base64, challenge) else {
        return false;
    };
    let payload = create_payload(message, nonce, app, callback_url);
    let mut borsh_payload: Vec<u8> = Vec::new();
    payload.serialize(&mut borsh_payload).unwrap();

    let payload_hash = hash_bytes(&borsh_payload);

    verify(public_key_str, &payload_hash, signature_base64).is_ok()
}

/// Checks if a given NEAR key exists for the provided account using the NEAR RPC.
///
/// # Arguments
/// * `current_near_root_key` - The public key to check.
/// * `account_id` - The NEAR account ID.
/// * `rpc_url` - The NEAR RPC URL to query.
///
/// # Returns
/// * `Ok(true)` - If the key exists.
/// * `Ok(false)` - If the key does not exist.
/// * `Err(ApiError)` - If the RPC request fails or the response contains an error.
pub async fn has_near_key(
    current_near_root_key: &str,
    account_id: &str,
    rpc_url: &str,
) -> EyreResult<bool, ApiError> {
    let client = Client::new();
    let body = RpcRequest {
        jsonrpc: "2.0".to_owned(),
        id: "dontcare".to_owned(),
        method: "query".to_owned(),
        params: RpcParams {
            request_type: "view_access_key".to_owned(),
            finality: "final".to_owned(),
            account_id: account_id.to_owned(),
            public_key: current_near_root_key.to_owned(),
        },
    };

    let response = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("Request failed: {e}"),
        })?
        .json::<NearJsonRpcResponse<ResultDataWithPermission>>()
        .await
        .map_err(|e| ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("Failed to parse response: {e}"),
        })?;

    // Check if there is a top-level error
    if let Some(ref error) = response.error {
        info!("Top-level error found: {:?}", error);
        return Err(ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("Top-level error: {}", error.message),
        });
    }

    // Check for an error within the result object
    if let Some(result) = &response.result {
        if let Some(error) = result.error.as_deref() {
            info!("Error within result: {}", error);
            return Err(ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: format!("Result error: {error}"),
            });
        }

        // If a valid key is found, return true
        return Ok(true);
    }
    Ok(false) // Return false if no matches are found
}

enum Encoding {
    Base64,
    Base58,
}

/// Decodes a base58 or base64-encoded string into a fixed-size array.
///
/// # Arguments
/// * `encoding` - The encoding used (Base58 or Base64).
/// * `encoded` - The string to decode.
///
/// # Returns
/// * `Ok([u8; N])` - The decoded array of bytes.
/// * `Err(Report)` - If the decoding fails or the size is incorrect.
fn decode_to_fixed_array<const N: usize>(
    encoding: &Encoding,
    encoded: &str,
) -> EyreResult<[u8; N]> {
    let decoded_vec = match encoding {
        Encoding::Base58 => bs58::decode(encoded)
            .into_vec()
            .map_err(|e| Report::new(e))?,
        Encoding::Base64 => STANDARD.decode(encoded).map_err(|e| Report::new(e))?,
    };

    let fixed_array: [u8; N] = decoded_vec
        .try_into()
        .map_err(|_| Report::msg("Incorrect length"))?;
    Ok(fixed_array)
}

/// Verifies the signature for a given message using the provided public key.
///
/// # Arguments
/// * `public_key_str` - The public key as a string.
/// * `message` - The message bytes to verify.
/// * `signature` - The base64-encoded signature to verify.
///
/// # Returns
/// * `Ok(())` - If the signature is valid.
/// * `Err(Report)` - If the verification fails.
fn verify(public_key_str: &str, message: &[u8], signature: &str) -> EyreResult<()> {
    let encoded_key = public_key_str.trim_start_matches("ed25519:");

    let decoded_key: [u8; 32] =
        decode_to_fixed_array(&Encoding::Base58, encoded_key).map_err(|e| eyre!(e))?;
    let vk = VerifyingKey::from_bytes(&decoded_key).map_err(|e| eyre!(e))?;

    let decoded_signature: [u8; 64] =
        decode_to_fixed_array(&Encoding::Base64, signature).map_err(|e| eyre!(e))?;
    let signature = Signature::from_bytes(&decoded_signature);

    vk.verify(message, &signature).map_err(|e| eyre!(e))?;

    Ok(())
}

/// Represents the payload structure that contains a message, nonce, recipient, and optional callback URL.
///
/// # Fields
/// * `tag` - A tag to identify the payload type.
/// * `message` - The message to be sent.
/// * `nonce` - A 32-byte nonce for the message.
/// * `recipient` - The recipient of the message.
/// * `callback_url` - An optional callback URL for the message.
#[derive(BorshSerialize)]
struct Payload {
    tag: u32,
    message: String,
    nonce: [u8; 32],
    recipient: String,
    callback_url: Option<String>,
}
