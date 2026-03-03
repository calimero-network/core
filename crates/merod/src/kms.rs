//! KMS client for fetching storage encryption keys.
//!
//! This module handles communication with KMS services to obtain storage
//! encryption keys using TDX attestation. Currently supports Phala Cloud KMS.

use base64::Engine;
use calimero_config::{KmsAttestationConfig, KmsConfig, PhalaKmsConfig};
use calimero_tee_attestation::{
    generate_attestation, is_mock_quote, verify_attestation, verify_mock_attestation,
    VerificationResult,
};
use eyre::{bail, Context, Result};
use libp2p::identity::Keypair;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};
use url::Url;

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

/// Request body for KMS self-attestation endpoint.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhalaKmsAttestRequest {
    nonce_b64: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    binding_b64: Option<String>,
}

/// Response body from KMS self-attestation endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhalaKmsAttestResponse {
    quote_b64: String,
    report_data_hex: String,
}

#[derive(Debug, Clone)]
struct NormalizedKmsAttestationPolicy {
    accept_mock: bool,
    allowed_tcb_statuses: Vec<String>,
    allowed_mrtd: Vec<String>,
    allowed_rtmr0: Vec<String>,
    allowed_rtmr1: Vec<String>,
    allowed_rtmr2: Vec<String>,
    allowed_rtmr3: Vec<String>,
    binding: [u8; 32],
    binding_b64: Option<String>,
}

/// Fetch the storage encryption key using the configured KMS provider.
///
/// Returns an error if no KMS provider is configured (incomplete TEE configuration)
/// or if key fetching fails.
///
/// # Arguments
/// * `kms_config` - KMS configuration specifying which provider to use
/// * `peer_id` - The peer ID string (base58 encoded)
/// * `identity` - Local node identity keypair used to sign challenge payloads
pub async fn fetch_storage_key(
    kms_config: &KmsConfig,
    peer_id: &str,
    identity: &Keypair,
) -> Result<Vec<u8>> {
    if let Some(ref phala_config) = kms_config.phala {
        info!("Using Phala Cloud KMS");
        let key = fetch_from_phala(phala_config, peer_id, identity).await?;
        Ok(key)
    } else {
        bail!(
            "TEE is enabled but no KMS provider is configured. \
             Please configure [tee.kms.phala] in your config.toml to enable storage encryption. \
             Running a TEE node without storage encryption is not supported."
        );
    }
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
/// * `phala_config` - Phala KMS configuration
/// * `peer_id` - The peer ID string (base58 encoded)
/// * `identity` - Local node identity keypair used to sign challenge payloads
///
/// # Returns
/// The storage encryption key bytes (hex-decoded from KMS response).
async fn fetch_from_phala(
    phala_config: &PhalaKmsConfig,
    peer_id: &str,
    identity: &Keypair,
) -> Result<Vec<u8>> {
    info!(%peer_id, "Fetching storage key from KMS");

    // Build endpoint URLs - ensure trailing slash to prevent Url::join path replacement.
    let base_url = ensure_trailing_slash(&phala_config.url);
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

    if phala_config.attestation.enabled {
        verify_kms_attestation(&client, &base_url, &phala_config.attestation).await?;
    }

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

async fn verify_kms_attestation(
    client: &reqwest::Client,
    base_url: &Url,
    attestation_config: &KmsAttestationConfig,
) -> Result<()> {
    let policy = normalize_kms_attestation_policy(attestation_config)?;
    let attest_endpoint = base_url
        .join("attest")
        .context("Failed to build KMS attest endpoint URL")?;

    let mut nonce = [0u8; 32];
    OsRng.fill_bytes(&mut nonce);

    let expected_report_data = build_kms_attestation_report_data(&nonce, &policy.binding);

    let request = PhalaKmsAttestRequest {
        nonce_b64: base64::engine::general_purpose::STANDARD.encode(nonce),
        binding_b64: policy.binding_b64.clone(),
    };

    info!(%attest_endpoint, "Verifying KMS self-attestation before key request");
    let attest_response = request_kms_attestation(client, &attest_endpoint, &request).await?;
    let (quote_bytes, report_data_bytes) = decode_kms_attestation_response(&attest_response)?;

    if report_data_bytes.len() != 64 {
        bail!(
            "KMS attest reportDataHex must be 64 bytes, got {}",
            report_data_bytes.len()
        );
    }

    if report_data_bytes.as_slice() != expected_report_data {
        bail!(
            "KMS attest reportData mismatch (nonce/binding mismatch or tampered response payload)"
        );
    }

    let verification_result = if is_mock_quote(&quote_bytes) {
        if !policy.accept_mock {
            bail!("KMS returned mock attestation quote, but attestation.accept_mock is disabled");
        }

        warn!("Accepting mock KMS attestation quote - this is insecure and for development only");
        verify_mock_attestation(&quote_bytes, &nonce, Some(&policy.binding))
            .context("Failed to verify mock KMS attestation")?
    } else {
        verify_attestation(&quote_bytes, &nonce, Some(&policy.binding))
            .await
            .context("Failed to verify KMS attestation quote")?
    };

    if !verification_result.is_valid() {
        bail!(
            "KMS attestation verification failed: quote_verified={}, nonce_verified={}, app_hash_verified={:?}",
            verification_result.quote_verified,
            verification_result.nonce_verified,
            verification_result.application_hash_verified
        );
    }

    enforce_kms_attestation_policy(&policy, &verification_result)?;
    info!("KMS self-attestation verified successfully");

    Ok(())
}

async fn request_kms_attestation(
    client: &reqwest::Client,
    attest_endpoint: &Url,
    request: &PhalaKmsAttestRequest,
) -> Result<PhalaKmsAttestResponse> {
    let response = client
        .post(attest_endpoint.as_str())
        .json(request)
        .send()
        .await
        .context("Failed to request KMS attestation")?;

    let status = response.status();
    if !status.is_success() {
        let error_body = response.text().await.unwrap_or_default();

        if let Ok(kms_error) = serde_json::from_str::<KmsErrorResponse>(&error_body) {
            let details = kms_error.details.unwrap_or_default();
            bail!(
                "KMS attestation request failed ({}): {} - {}",
                status,
                kms_error.error,
                details
            );
        }

        bail!(
            "KMS attestation request failed ({}): {}",
            status,
            error_body
        );
    }

    response
        .json()
        .await
        .context("Failed to parse KMS attest response")
}

fn decode_kms_attestation_response(
    attest_response: &PhalaKmsAttestResponse,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let quote_bytes = base64::engine::general_purpose::STANDARD
        .decode(&attest_response.quote_b64)
        .context("Failed to decode KMS quote from base64")?;
    let report_data_bytes = hex::decode(&attest_response.report_data_hex)
        .context("Failed to decode reportDataHex from KMS attest response")?;

    Ok((quote_bytes, report_data_bytes))
}

fn normalize_kms_attestation_policy(
    config: &KmsAttestationConfig,
) -> Result<NormalizedKmsAttestationPolicy> {
    config.validate_enabled_policy()?;

    let allowed_tcb_statuses = config
        .allowed_tcb_statuses
        .iter()
        .map(|status| status.trim().to_ascii_lowercase())
        .filter(|status| !status.is_empty())
        .collect::<Vec<_>>();

    let allowed_mrtd = parse_measurement_allowlist(&config.allowed_mrtd, "allowed_mrtd")?;
    let allowed_rtmr0 = parse_measurement_allowlist(&config.allowed_rtmr0, "allowed_rtmr0")?;
    let allowed_rtmr1 = parse_measurement_allowlist(&config.allowed_rtmr1, "allowed_rtmr1")?;
    let allowed_rtmr2 = parse_measurement_allowlist(&config.allowed_rtmr2, "allowed_rtmr2")?;
    let allowed_rtmr3 = parse_measurement_allowlist(&config.allowed_rtmr3, "allowed_rtmr3")?;

    let binding = if let Some(binding_b64) = config.binding_b64.as_deref() {
        let binding_bytes = base64::engine::general_purpose::STANDARD
            .decode(binding_b64)
            .context("Failed to decode tee.kms.phala.attestation.binding_b64")?;
        binding_bytes.try_into().map_err(|_| {
            eyre::eyre!("tee.kms.phala.attestation.binding_b64 must decode to exactly 32 bytes")
        })?
    } else {
        default_kms_attestation_binding()
    };

    Ok(NormalizedKmsAttestationPolicy {
        accept_mock: config.accept_mock,
        allowed_tcb_statuses,
        allowed_mrtd,
        allowed_rtmr0,
        allowed_rtmr1,
        allowed_rtmr2,
        allowed_rtmr3,
        binding,
        binding_b64: config.binding_b64.clone(),
    })
}

fn parse_measurement_allowlist(values: &[String], field_name: &str) -> Result<Vec<String>> {
    let mut normalized_values = Vec::with_capacity(values.len());

    for raw in values {
        let normalized = normalize_measurement(raw);
        if normalized.is_empty() {
            continue;
        }

        let decoded = hex::decode(&normalized)
            .with_context(|| format!("{field_name} contains non-hex value: {raw}"))?;
        if decoded.len() != 48 {
            bail!(
                "{field_name} contains invalid measurement length (expected 48 bytes, got {})",
                decoded.len()
            );
        }

        normalized_values.push(normalized);
    }

    Ok(normalized_values)
}

fn default_kms_attestation_binding() -> [u8; 32] {
    Sha256::digest(b"mero-kms-phala-attest-v1").into()
}

fn build_kms_attestation_report_data(nonce: &[u8; 32], binding: &[u8; 32]) -> [u8; 64] {
    let mut report_data = [0u8; 64];
    report_data[..32].copy_from_slice(nonce);
    report_data[32..].copy_from_slice(binding);
    report_data
}

fn enforce_kms_attestation_policy(
    policy: &NormalizedKmsAttestationPolicy,
    verification_result: &VerificationResult,
) -> Result<()> {
    let actual_tcb_status = verification_result
        .tcb_status
        .clone()
        .ok_or_else(|| eyre::eyre!("KMS attestation did not include TCB status"))?;
    let normalized_tcb_status = actual_tcb_status.to_ascii_lowercase();
    if !policy
        .allowed_tcb_statuses
        .iter()
        .any(|allowed| allowed == &normalized_tcb_status)
    {
        bail!(
            "KMS TCB status '{}' is not allowed. Allowed: {}",
            actual_tcb_status,
            policy.allowed_tcb_statuses.join(", ")
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
    actual_measurement: &str,
    allowed_measurements: &[String],
) -> Result<()> {
    if allowed_measurements.is_empty() {
        return Ok(());
    }

    let normalized_actual = normalize_measurement(actual_measurement);
    if allowed_measurements
        .iter()
        .any(|allowed| allowed == &normalized_actual)
    {
        return Ok(());
    }

    bail!("{label} '{}' is not in allowlist", normalized_actual);
}

fn normalize_measurement(value: &str) -> String {
    value.trim().trim_start_matches("0x").to_ascii_lowercase()
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

    #[test]
    fn test_parse_measurement_allowlist_accepts_prefixed_hex() {
        let values = vec![format!("0x{}", "ab".repeat(48))];
        let parsed = parse_measurement_allowlist(&values, "allowed_mrtd").unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], "ab".repeat(48));
    }

    #[test]
    fn test_parse_measurement_allowlist_rejects_invalid_length() {
        let values = vec!["ff".repeat(47)];
        assert!(parse_measurement_allowlist(&values, "allowed_mrtd").is_err());
    }

    #[test]
    fn test_normalize_kms_attestation_policy_requires_mrtd() {
        let mut cfg = KmsAttestationConfig::default();
        cfg.enabled = true;
        let result = normalize_kms_attestation_policy(&cfg);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_kms_attestation_report_data_layout() {
        let nonce = [0x11; 32];
        let binding = [0x22; 32];

        let report_data = build_kms_attestation_report_data(&nonce, &binding);
        assert_eq!(&report_data[..32], &nonce);
        assert_eq!(&report_data[32..], &binding);
    }
}
