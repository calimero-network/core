//! KMS client for fetching storage encryption keys.
//!
//! This module handles communication with KMS services to obtain storage
//! encryption keys using TDX attestation. Currently supports Phala Cloud KMS.
//!
//! When MERO_TEE_VERSION or MERO_KMS_VERSION is set, merod verifies the KMS
//! via POST /attest before requesting keys, using policy fetched from the release.

use base64::Engine;
use calimero_config::KmsConfig;
use calimero_tee_attestation::{
    generate_attestation, is_mock_quote, verify_attestation, verify_mock_attestation,
};
use eyre::{bail, Context, Result};
use libp2p::identity::Keypair;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, error, info, warn};
use url::Url;

use crate::kms_policy::KmsAttestationPolicy;

/// Request body for the Phala KMS challenge endpoint.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhalaChallengeRequest {
    peer_id: String,
}

/// Response body from the Phala KMS challenge endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhalaChallengeResponse {
    challenge_id: String,
    nonce_b64: String,
}

/// Request body for the Phala KMS get-key endpoint.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhalaGetKeyRequest {
    challenge_id: String,
    quote_b64: String,
    peer_id: String,
    peer_public_key_b64: String,
    signature_b64: String,
}

/// Response body from the Phala KMS get-key endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhalaGetKeyResponse {
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

/// Request body for the KMS attest endpoint.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct KmsAttestRequest {
    nonce_b64: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    binding_b64: Option<String>,
}

/// Response body from the KMS attest endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KmsAttestResponse {
    quote_b64: String,
    #[allow(dead_code)] // Required for deserialization; used for audit/debug if needed
    report_data_hex: String,
}

/// Fetch the storage encryption key using the configured KMS provider.
///
/// When `policy` is provided (from MERO_TEE_VERSION / MERO_KMS_VERSION), verifies
/// the KMS via POST /attest before requesting keys.
///
/// Returns an error if no KMS provider is configured (incomplete TEE configuration)
/// or if key fetching fails.
///
/// # Arguments
/// * `kms_config` - KMS configuration specifying which provider to use
/// * `peer_id` - The peer ID string (base58 encoded)
/// * `identity` - Local node identity keypair used to sign challenge payloads
/// * `policy` - Optional attestation policy from release (when MERO_TEE_VERSION set)
pub async fn fetch_storage_key(
    kms_config: &KmsConfig,
    peer_id: &str,
    identity: &Keypair,
    policy: Option<&KmsAttestationPolicy>,
) -> Result<Vec<u8>> {
    if let Some(ref phala_config) = kms_config.phala {
        info!("Using Phala Cloud KMS");
        if let Some(p) = policy {
            verify_kms_attestation(&phala_config.url, p).await?;
        }
        let key = fetch_from_phala(&phala_config.url, peer_id, identity).await?;
        Ok(key)
    } else {
        bail!(
            "TEE is enabled but no KMS provider is configured. \
             Please configure [tee.kms.phala] in your config.toml to enable storage encryption. \
             Running a TEE node without storage encryption is not supported."
        );
    }
}

/// Verify KMS via POST /attest before requesting keys.
///
/// Calls KMS /attest, verifies the quote, and enforces measurement policy.
async fn verify_kms_attestation(kms_url: &Url, policy: &KmsAttestationPolicy) -> Result<()> {
    info!("Verifying KMS attestation before key fetch");

    let base_url = ensure_trailing_slash(kms_url);
    let attest_endpoint = base_url
        .join("attest")
        .context("Failed to build KMS attest endpoint URL")?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")?;

    let mut nonce = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut nonce);
    let nonce_b64 = base64::engine::general_purpose::STANDARD.encode(nonce);

    let request = KmsAttestRequest {
        nonce_b64: nonce_b64.clone(),
        binding_b64: Some(policy.default_binding_b64.clone()),
    };

    let response = client
        .post(attest_endpoint.as_str())
        .json(&request)
        .send()
        .await
        .context("Failed to request KMS attestation")?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("KMS attest request failed ({}): {}", status, body);
    }

    let attest: KmsAttestResponse = response
        .json()
        .await
        .context("Failed to parse KMS attest response")?;

    let quote_bytes = base64::engine::general_purpose::STANDARD
        .decode(&attest.quote_b64)
        .context("Failed to decode KMS quote from base64")?;

    let binding_bytes = base64::engine::general_purpose::STANDARD
        .decode(&policy.default_binding_b64)
        .context("Invalid policy default_binding_b64")?;
    let binding: [u8; 32] = binding_bytes
        .try_into()
        .map_err(|_| eyre::eyre!("Policy default_binding_b64 must be 32 bytes"))?;

    let verification_result = if is_mock_quote(&quote_bytes) {
        warn!("KMS returned mock attestation - skipping measurement policy");
        verify_mock_attestation(&quote_bytes, &nonce, Some(&binding))
            .context("KMS mock attestation verification failed")?
    } else {
        verify_attestation(&quote_bytes, &nonce, Some(&binding))
            .await
            .context("KMS attestation verification failed")?
    };

    if !verification_result.is_valid() {
        error!(
            quote_verified = verification_result.quote_verified,
            nonce_verified = verification_result.nonce_verified,
            "KMS attestation verification failed"
        );
        bail!("KMS attestation verification failed");
    }

    enforce_attestation_policy(policy, &verification_result)?;
    info!("KMS attestation verified successfully");
    Ok(())
}

fn enforce_attestation_policy(
    policy: &KmsAttestationPolicy,
    verification_result: &calimero_tee_attestation::VerificationResult,
) -> Result<()> {
    let actual_tcb_status = verification_result
        .tcb_status
        .as_ref()
        .ok_or_else(|| eyre::eyre!("Quote verification did not provide a TCB status"))?;
    let normalized_tcb = actual_tcb_status.to_lowercase();

    if !policy
        .allowed_tcb_statuses
        .iter()
        .any(|a| a == &normalized_tcb)
    {
        bail!(
            "KMS TCB status '{}' is not allowed. Allowed: {:?}",
            actual_tcb_status,
            policy.allowed_tcb_statuses
        );
    }

    let body = &verification_result.quote.body;
    enforce_measurement_allowlist("MRTD", &body.mrtd, &policy.allowed_mrtd)?;
    enforce_measurement_allowlist("RTMR0", &body.rtmr0, &policy.allowed_rtmr0)?;
    enforce_measurement_allowlist("RTMR1", &body.rtmr1, &policy.allowed_rtmr1)?;
    enforce_measurement_allowlist("RTMR2", &body.rtmr2, &policy.allowed_rtmr2)?;
    enforce_measurement_allowlist("RTMR3", &body.rtmr3, &policy.allowed_rtmr3)?;
    Ok(())
}

fn enforce_measurement_allowlist(
    label: &str,
    actual: &str,
    allowed: &[String],
) -> Result<()> {
    if allowed.is_empty() {
        return Ok(());
    }
    let normalized = actual.trim().trim_start_matches("0x").to_lowercase();
    if allowed.iter().any(|a| a == &normalized) {
        return Ok(());
    }
    bail!("KMS {} '{}' is not in allowlist", label, normalized)
}

/// Fetch the storage encryption key from Phala Cloud KMS (mero-kms-phala).
///
/// This function:
/// 1. Requests a one-time challenge nonce from KMS
/// 2. Generates a TDX attestation with challenge nonce in report_data[0..32]
///    and SHA256(peer_id) in report_data[32..64]
/// 3. Signs challenge + quote hash with node identity key
/// 4. Sends the signed attestation request to KMS
/// 5. Returns the encryption key bytes
///
/// # Arguments
/// * `kms_url` - Base URL of the mero-kms-phala service
/// * `peer_id` - The peer ID string (base58 encoded)
/// * `identity` - Local node identity keypair used to sign challenge payloads
///
/// # Returns
/// The storage encryption key bytes (hex-decoded from KMS response).
async fn fetch_from_phala(kms_url: &Url, peer_id: &str, identity: &Keypair) -> Result<Vec<u8>> {
    info!(%peer_id, "Fetching storage key from KMS");

    // Build endpoint URLs - ensure trailing slash to prevent Url::join path replacement.
    let base_url = ensure_trailing_slash(kms_url);
    let challenge_endpoint = base_url
        .join("challenge")
        .context("Failed to build KMS challenge endpoint URL")?;
    let key_endpoint = base_url
        .join("get-key")
        .context("Failed to build KMS get-key endpoint URL")?;

    // Build HTTP client once and reuse for both requests.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")?;

    // 1) Request one-time challenge nonce.
    info!(%challenge_endpoint, "Requesting key release challenge from KMS");
    let challenge_request = PhalaChallengeRequest {
        peer_id: peer_id.to_string(),
    };
    let challenge_response = client
        .post(challenge_endpoint.as_str())
        .json(&challenge_request)
        .send()
        .await
        .context("Failed to request challenge from KMS")?;

    let challenge_status = challenge_response.status();
    if !challenge_status.is_success() {
        let error_body = challenge_response.text().await.unwrap_or_default();
        if let Ok(kms_error) = serde_json::from_str::<KmsErrorResponse>(&error_body) {
            let details = kms_error.details.unwrap_or_default();
            bail!(
                "KMS challenge request failed ({}): {} - {}",
                challenge_status,
                kms_error.error,
                details
            );
        }
        bail!(
            "KMS challenge request failed ({}): {}",
            challenge_status,
            error_body
        );
    }

    let challenge: PhalaChallengeResponse = challenge_response
        .json()
        .await
        .context("Failed to parse KMS challenge response")?;
    let challenge_nonce_vec = base64::engine::general_purpose::STANDARD
        .decode(&challenge.nonce_b64)
        .context("Failed to decode challenge nonce from base64")?;
    let challenge_nonce: [u8; 32] = challenge_nonce_vec
        .try_into()
        .map_err(|_| eyre::eyre!("Challenge nonce must be exactly 32 bytes"))?;

    debug!(
        challenge_id = %challenge.challenge_id,
        challenge_nonce = %hex::encode(challenge_nonce),
        "Received KMS challenge"
    );

    // 2) Create report_data with challenge nonce in [0..32] and SHA256(peer_id) in [32..64].
    let peer_id_hash = hash_peer_id(peer_id);
    debug!(
        peer_id_hash = %hex::encode(&peer_id_hash),
        "Generated peer ID hash for attestation"
    );

    let mut report_data = [0u8; 64];
    report_data[..32].copy_from_slice(&challenge_nonce);
    report_data[32..].copy_from_slice(&peer_id_hash);

    // 3) Generate attestation
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

    // 4) Sign challenge payload using node identity key.
    let signature_payload = build_signature_payload(
        &challenge.challenge_id,
        &challenge_nonce,
        &attestation.quote_bytes,
        peer_id,
    )?;
    let signature = identity
        .sign(&signature_payload)
        .context("Failed to sign KMS challenge payload with node identity key")?;
    let peer_public_key = identity.public().encode_protobuf();

    // 5) Build signed key request.
    let request = PhalaGetKeyRequest {
        challenge_id: challenge.challenge_id,
        quote_b64: attestation.quote_b64,
        peer_id: peer_id.to_string(),
        peer_public_key_b64: base64::engine::general_purpose::STANDARD.encode(peer_public_key),
        signature_b64: base64::engine::general_purpose::STANDARD.encode(signature),
    };

    info!(%key_endpoint, "Sending signed key request to KMS");
    let response = client
        .post(key_endpoint.as_str())
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
    let response: PhalaGetKeyResponse = response
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

fn build_signature_payload(
    challenge_id: &str,
    challenge_nonce: &[u8; 32],
    quote_bytes: &[u8],
    peer_id: &str,
) -> Result<Vec<u8>> {
    let quote_hash = Sha256::digest(quote_bytes);
    let payload = serde_json::json!({
        "challengeId": challenge_id,
        "challengeNonceHex": hex::encode(challenge_nonce),
        "quoteHashHex": hex::encode(quote_hash),
        "peerId": peer_id,
    });
    serde_json::to_vec(&payload).context("Failed to serialize challenge signature payload")
}

/// Hash a peer ID string to create a 32-byte value for attestation.
///
/// This must match the hashing used by the KMS service.
fn hash_peer_id(peer_id: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(peer_id.as_bytes());
    hasher.finalize().into()
}

/// Ensure URL has a trailing slash to prevent `Url::join` from replacing the last path segment.
///
/// `Url::join` has counter-intuitive behavior: if the base URL lacks a trailing slash,
/// it replaces the last path segment. For example:
/// - `http://host/api/v1`.join("get-key") => `http://host/api/get-key` (wrong!)
/// - `http://host/api/v1/`.join("get-key") => `http://host/api/v1/get-key` (correct)
fn ensure_trailing_slash(url: &Url) -> Url {
    let mut url = url.clone();
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    url
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

    #[test]
    fn test_ensure_trailing_slash() {
        // URL without trailing slash should get one added
        let url = Url::parse("http://host/api/v1").unwrap();
        let fixed = ensure_trailing_slash(&url);
        assert_eq!(fixed.as_str(), "http://host/api/v1/");

        // URL with trailing slash should remain unchanged
        let url = Url::parse("http://host/api/v1/").unwrap();
        let fixed = ensure_trailing_slash(&url);
        assert_eq!(fixed.as_str(), "http://host/api/v1/");

        // Root URL should work
        let url = Url::parse("http://host").unwrap();
        let fixed = ensure_trailing_slash(&url);
        assert_eq!(fixed.as_str(), "http://host/");
    }

    #[test]
    fn test_url_join_with_trailing_slash() {
        // This test verifies that our fix works correctly
        let url_without_slash = Url::parse("http://host/api/v1").unwrap();
        let url_with_slash = ensure_trailing_slash(&url_without_slash);

        // Without the fix, this would produce http://host/api/get-key
        let endpoint = url_with_slash.join("get-key").unwrap();
        assert_eq!(endpoint.as_str(), "http://host/api/v1/get-key");
    }

    #[test]
    fn test_signature_payload_is_deterministic() {
        let challenge_id = "abc123";
        let challenge_nonce = [0x5a; 32];
        let quote_bytes = b"quote-bytes";
        let peer_id = "12D3KooWAbcdefghijklmnopqrstuvwxyz";

        let payload1 =
            build_signature_payload(challenge_id, &challenge_nonce, quote_bytes, peer_id).unwrap();
        let payload2 =
            build_signature_payload(challenge_id, &challenge_nonce, quote_bytes, peer_id).unwrap();

        assert_eq!(payload1, payload2);
    }
}
