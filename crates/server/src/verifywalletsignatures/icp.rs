//! This module provides functionality to verify signatures from Internet Computer using an Internet Identity (II).
//! It defines structures for delegation and signed delegation chains, and verifies the integrity of the delegations
//! and canister signatures.
use candid::Principal;
use ic_canister_sig_creation::{
    delegation_signature_msg, CanisterSigPublicKey, DELEGATION_SIG_DOMAIN,
};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::admin::service::ApiError;

/// A custom structure representing a `Delegation`, which includes a public key, an expiration time, and optional targets.
/// This struct is used to parse values from JSON, where the public key and expiration are provided as hex values.
///
/// # Fields
/// - `pubkey`: The public key as a `Vec<u8>`, parsed from hex in JSON.
/// - `expiration`: A `Vec<u8>` representing a Unix timestamp in nanoseconds, stored as a big-endian hex string.
/// - `targets`: An optional vector of `Vec<u8>`, which may include specific targets for the delegation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Delegation {
    #[serde(with = "hex::serde")]
    pub pubkey: Vec<u8>,
    #[serde(with = "hex::serde")]
    pub expiration: Vec<u8>, // Unix timestamp ns, u46 as BE-hex (that's how browser encodes it)
    pub targets: Option<Vec<Vec<u8>>>,
}

/// Represents a signed `Delegation` which includes the `Delegation` and its cryptographic signature.
///
/// # Fields
/// - `delegation`: The delegation details (`Delegation`).
/// - `signature`: The signature of the delegation, serialized as a hex string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
struct SignedDelegation {
    pub delegation: Delegation,
    #[serde(with = "hex::serde")]
    pub signature: Vec<u8>,
}

/// A chain of signed delegations along with a public key.
/// This structure is used to verify delegation authenticity within a chain.
///
/// # Fields
/// - `delegations`: A vector of signed delegations.
/// - `publicKey`: The public key that signs the delegations, serialized as a hex string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[allow(non_snake_case)]
struct DelegationChain {
    delegations: Vec<SignedDelegation>,
    #[serde(with = "hex::serde")]
    publicKey: Vec<u8>,
}

impl Delegation {
    /// Returns the expiration timestamp from the `Delegation` as a `u64`.
    ///
    /// # Panics
    /// This function will panic if the `expiration` vector cannot be converted into a `u64`.
    fn expiration(&self) -> u64 {
        let expiration_bytes: [u8; 8] = <[u8; 8]>::try_from(self.expiration.as_slice()).unwrap();
        u64::from_be_bytes(expiration_bytes)
    }
}

/// A constant array representing the Internet Computer (IC) root public key used in signature verification.
const IC_ROOT_PUBLIC_KEY: [u8; 96] = [
    129, 76, 14, 110, 199, 31, 171, 88, 59, 8, 189, 129, 55, 60, 37, 92, 60, 55, 27, 46, 132, 134,
    60, 152, 164, 241, 224, 139, 116, 35, 93, 20, 251, 93, 156, 12, 213, 70, 217, 104, 95, 145, 58,
    12, 11, 44, 197, 52, 21, 131, 191, 75, 67, 146, 228, 103, 219, 150, 214, 91, 155, 180, 203,
    113, 113, 18, 248, 71, 46, 13, 90, 77, 20, 80, 95, 253, 116, 132, 176, 18, 145, 9, 28, 95, 135,
    185, 136, 131, 70, 63, 152, 9, 26, 11, 170, 174,
];

/// Verifies the Internet Identity (II) signature from a provided challenge, delegation chain, and II canister ID.
///
/// # Arguments
/// - `challenge`: The challenge data (public key) to verify the signature against.
/// - `signed_delegation_chain_json`: A JSON string representing the signed delegation chain.
/// - `ii_canister_id`: The ID of the II canister from which the delegation originates.
///
/// # Returns
/// - `Ok(())`: If the signature and delegation chain are successfully verified.
/// - `Err(ApiError)`: If any validation step fails, such as parsing errors, signature mismatches, or invalid delegation chains.
///
/// # Errors
/// This function will return an `ApiError` in case of issues like invalid input, signature mismatches, or verification failure.
pub async fn verify_internet_identity_signature(
    challenge: &[u8],
    signed_delegation_chain_json: Value,
    ii_canister_id: &str,
) -> Result<(), ApiError> {
    // Deserialize the `Value` into `DelegationChain`
    let signed_delegation_chain: DelegationChain =
        serde_json::from_value(signed_delegation_chain_json).map_err(|e| ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("Error parsing delegation_chain: {}", e),
        })?;

    let signed_delegation = &signed_delegation_chain.delegations[0];
    let delegation = &signed_delegation.delegation;

    // Checks if the provided challenge (public key) matches the delegation's public key
    if delegation.pubkey != challenge {
        return Err(ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!(
                "delegation.pubkey {} does not match the challenge",
                hex::encode(delegation.pubkey.clone())
            ),
        });
    }

    // Validates the canister signature public key and compares it to the II canister ID
    let cs_pk = CanisterSigPublicKey::try_from(signed_delegation_chain.publicKey.as_slice())
        .map_err(|e| ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("Invalid publicKey in delegation chain: {}", e),
        })?;

    let expected_ii_canister_id = Principal::from_text(ii_canister_id).map_err(|e| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("Invalid ii_canister_id: {}", e),
    })?;

    if cs_pk.canister_id != expected_ii_canister_id {
        return Err(ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!(
                "Delegation's signing canister {} does not match II canister id {}",
                cs_pk.canister_id, expected_ii_canister_id
            ),
        });
    }

    // Verifies the canister signature by checking the message and the provided signature
    let message = msg_with_domain(
        DELEGATION_SIG_DOMAIN,
        &delegation_signature_msg(
            delegation.pubkey.as_slice(),
            delegation.expiration(),
            delegation.targets.as_ref(),
        ),
    );

    ic_signature_verification::verify_canister_sig(
        message.as_slice(),
        signed_delegation.signature.as_slice(),
        &cs_pk.to_der(),
        &IC_ROOT_PUBLIC_KEY,
    )
    .map_err(|e| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("Invalid canister signature: {}", e),
    })?;

    Ok(())
}

/// Combines a domain separator with the provided message, used for signing purposes.
///
/// # Arguments
/// - `sep`: The domain separator to prepend.
/// - `bytes`: The message to append.
///
/// # Returns
/// A vector combining the domain separator and the message.
fn msg_with_domain(sep: &[u8], bytes: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(1 + sep.len() + bytes.len()); // Pre-allocate space for efficiency
    msg.push(sep.len() as u8);
    msg.extend_from_slice(sep);
    msg.extend_from_slice(bytes);
    msg
}
