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
use serde_json::json;
use sha2::{Digest, Sha256};
use tracing::info;

use crate::admin::service::ApiError;

#[derive(Debug, Serialize, Deserialize)]
struct NearJsonRpcResponse<T> {
    jsonrpc: String,
    result: Option<T>,
    error: Option<NearJsonRpcError>, // Top-level error
    id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct NearJsonRpcError {
    code: i64,
    message: String,
    data: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ResultDataWithPermission {
    block_hash: String,
    block_height: u64,
    nonce: Option<u64>,
    permission: Option<Permission>,
    error: Option<String>, // Result-level error
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum Permission {
    FunctionCall(FunctionCall),
    FullAccess(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct FunctionCall {
    allowance: String,
    receiver_id: String,
    method_names: Vec<String>,
}

// Define the outermost structure for the RPC request
#[derive(Serialize, Deserialize, Debug)]
struct RpcRequest {
    jsonrpc: String,
    id: String,
    method: String,
    params: RpcParams,
}

// Define the structure for the params field
#[derive(Serialize, Deserialize, Debug)]
struct RpcParams {
    request_type: String,
    finality: String,
    account_id: String,
    public_key: String,
}

fn create_payload(message: &str, nonce: [u8; 32], recipient: &str, callback_url: &str) -> Payload {
    Payload {
        tag: 2_147_484_061,
        message: message.to_owned(),
        nonce,
        recipient: recipient.to_owned(),
        callback_url: Some(callback_url.to_owned()),
    }
}

fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();

    hasher.update(bytes);

    let result = hasher.finalize();

    let mut hash_array = [0_u8; 32];
    hash_array.copy_from_slice(&result);

    hash_array
}

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

// Check if root key belongs to the account
pub async fn has_near_key(
    current_near_root_key: &str,
    account_id: &str,
    rpc_url: &str,
) -> EyreResult<bool, ApiError> {
    let client = Client::new();
    let body = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: "dontcare".to_string(),
        method: "query".to_string(),
        params: RpcParams {
            request_type: "view_access_key".to_string(),
            finality: "final".to_string(),
            account_id: account_id.to_string(),
            public_key: current_near_root_key.to_string(),
        },
    };

    let response = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("Request failed: {}", e),
        })?
        .json::<NearJsonRpcResponse<ResultDataWithPermission>>()
        .await
        .map_err(|e| ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("Failed to parse response: {}", e),
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
                message: format!("Result error: {}", error),
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

#[derive(BorshSerialize)]
struct Payload {
    tag: u32,
    message: String,
    nonce: [u8; 32],
    recipient: String,
    callback_url: Option<String>,
}
