//! KMS client for fetching storage encryption keys.
//!
//! This module handles communication with the mero-kms-phala service
//! to obtain storage encryption keys using TDX attestation.

use calimero_tee_attestation::generate_attestation;
use eyre::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};
use url::Url;

/// Request body for the KMS get-key endpoint.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetKeyRequest {
    quote_b64: String,
    peer_id: String,
}

/// Response body from the KMS get-key endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetKeyResponse {
    key: String,
}

/// Error response from the KMS service.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KmsErrorResponse {
    error: String,
    #[serde(default)]
    details: Option<String>,
}

/// Fetch the storage encryption key from the KMS service.
///
/// This function:
/// 1. Generates a TDX attestation with SHA256(peer_id) in report_data[0..32]
/// 2. Sends the attestation to the KMS service
/// 3. Returns the encryption key bytes
///
/// # Arguments
/// * `kms_url` - Base URL of the KMS service
/// * `peer_id` - The peer ID string (base58 encoded)
///
/// # Returns
/// The storage encryption key bytes (hex-decoded from KMS response).
pub async fn fetch_storage_key(kms_url: &Url, peer_id: &str) -> Result<Vec<u8>> {
    info!(%peer_id, "Fetching storage key from KMS");

    // Create report_data with SHA256(peer_id) in first 32 bytes
    let peer_id_hash = hash_peer_id(peer_id);
    debug!(
        peer_id_hash = %hex::encode(&peer_id_hash),
        "Generated peer ID hash for attestation"
    );

    let mut report_data = [0u8; 64];
    report_data[..32].copy_from_slice(&peer_id_hash);

    // Generate attestation
    let attestation =
        generate_attestation(report_data).context("Failed to generate TDX attestation")?;

    if attestation.is_mock {
        warn!("Generated MOCK attestation - this will only work if KMS accepts mock attestations");
    } else {
        info!("Generated real TDX attestation");
    }

    debug!(
        quote_len = attestation.quote_bytes.len(),
        is_mock = attestation.is_mock,
        "Attestation generated"
    );

    // Build request
    let request = GetKeyRequest {
        quote_b64: attestation.quote_b64,
        peer_id: peer_id.to_string(),
    };

    // Build endpoint URL
    let endpoint = kms_url
        .join("/get-key")
        .context("Failed to build KMS endpoint URL")?;

    info!(%endpoint, "Sending key request to KMS");

    // Send request
    let client = reqwest::Client::new();
    let response = client
        .post(endpoint.as_str())
        .json(&request)
        .send()
        .await
        .context("Failed to send request to KMS")?;

    let status = response.status();

    if !status.is_success() {
        let error_body = response.text().await.unwrap_or_default();

        // Try to parse as KMS error response
        if let Ok(kms_error) = serde_json::from_str::<KmsErrorResponse>(&error_body) {
            let details = kms_error.details.unwrap_or_default();
            bail!(
                "KMS request failed ({}): {} - {}",
                status,
                kms_error.error,
                details
            );
        }

        bail!("KMS request failed ({}): {}", status, error_body);
    }

    // Parse response
    let response: GetKeyResponse = response
        .json()
        .await
        .context("Failed to parse KMS response")?;

    // Decode hex-encoded key from KMS
    let key_bytes = hex::decode(&response.key).context("Failed to decode key from hex")?;

    info!(
        key_len = key_bytes.len(),
        "Successfully fetched storage key from KMS"
    );

    Ok(key_bytes)
}

/// Hash a peer ID string to create a 32-byte value for attestation.
///
/// This must match the hashing used by the KMS service.
fn hash_peer_id(peer_id: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(peer_id.as_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_peer_id() {
        let peer_id = "12D3KooWAbcdefghijklmnopqrstuvwxyz";
        let hash = hash_peer_id(peer_id);
        assert_eq!(hash.len(), 32);

        // Same peer_id should produce same hash
        let hash2 = hash_peer_id(peer_id);
        assert_eq!(hash, hash2);

        // Different peer_id should produce different hash
        let hash3 = hash_peer_id("12D3KooWDifferentPeerId");
        assert_ne!(hash, hash3);
    }
}
