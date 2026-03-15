//! KMS client for fetching storage encryption keys.
//!
//! This module handles communication with KMS services to obtain storage
//! encryption keys using TDX attestation. Currently supports Phala Cloud KMS.
//!
//! When MERO_KMS_RELEASE_TAG, MERO_KMS_VERSION, or MERO_TEE_VERSION is set,
//! merod verifies the KMS via POST /attest before requesting keys, using
//! policy fetched from the release.

use base64::Engine;
use calimero_config::{
    normalize_attestation_measurement, KmsAttestationConfig, KmsConfig, PhalaKmsConfig,
};
use calimero_tee_attestation::{
    generate_attestation, is_mock_quote, verify_attestation, verify_mock_attestation,
    VerificationResult,
};
use camino::{Utf8Path, Utf8PathBuf};
use eyre::{bail, Context, Result};
use libp2p::identity::Keypair;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
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

#[derive(Debug, Clone)]
struct KmsHttpFailure {
    endpoint: &'static str,
    status: reqwest::StatusCode,
    kms_error: Option<String>,
    details: String,
}

impl KmsHttpFailure {
    fn from_response(endpoint: &'static str, status: reqwest::StatusCode, body: &str) -> Self {
        if let Ok(kms_error) = serde_json::from_str::<KmsErrorResponse>(body) {
            return Self {
                endpoint,
                status,
                kms_error: Some(kms_error.error),
                details: kms_error.details.unwrap_or_default(),
            };
        }

        Self {
            endpoint,
            status,
            kms_error: None,
            details: body.to_owned(),
        }
    }
}

impl std::fmt::Display for KmsHttpFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(kms_error) = self.kms_error.as_deref() {
            return write!(
                f,
                "KMS {} request failed ({}): {} - {}",
                self.endpoint, self.status, kms_error, self.details
            );
        }

        write!(
            f,
            "KMS {} request failed ({}): {}",
            self.endpoint, self.status, self.details
        )
    }
}

impl std::error::Error for KmsHttpFailure {}

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

#[derive(Debug, Clone, Copy)]
enum KeyFetchAttestationMode {
    /// Use attestation settings configured in `config.toml`.
    UseConfigPolicy,
    /// Skip config attestation because release-policy verification already ran.
    AlreadyVerifiedFromReleasePolicy,
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

#[derive(Debug, Clone, Deserialize)]
struct ExternalKmsAttestationPolicy {
    #[serde(default)]
    allowed_tcb_statuses: Option<Vec<String>>,
    #[serde(default)]
    allowed_mrtd: Option<Vec<String>>,
    #[serde(default)]
    allowed_rtmr0: Option<Vec<String>>,
    #[serde(default)]
    allowed_rtmr1: Option<Vec<String>>,
    #[serde(default)]
    allowed_rtmr2: Option<Vec<String>>,
    #[serde(default)]
    allowed_rtmr3: Option<Vec<String>>,
    #[serde(default)]
    binding_b64: Option<String>,
    // Canonical mero-tee policy schema nests allowlists under `policy`.
    #[serde(default)]
    policy: Option<ExternalKmsAttestationPolicyValues>,
    // Canonical mero-tee policy schema nests default binding under `kms`.
    #[serde(default)]
    kms: Option<ExternalKmsAttestationPolicyKms>,
}

#[derive(Debug, Clone, Deserialize)]
struct ExternalKmsAttestationPolicyValues {
    #[serde(default)]
    allowed_tcb_statuses: Option<Vec<String>>,
    #[serde(default)]
    allowed_mrtd: Option<Vec<String>>,
    #[serde(default)]
    allowed_rtmr0: Option<Vec<String>>,
    #[serde(default)]
    allowed_rtmr1: Option<Vec<String>>,
    #[serde(default)]
    allowed_rtmr2: Option<Vec<String>>,
    #[serde(default)]
    allowed_rtmr3: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
struct ExternalKmsAttestationPolicyKms {
    #[serde(default)]
    default_binding_b64: Option<String>,
}

const EXTERNAL_POLICY_ALLOWED_DIRS: &[&str] = &["/etc/calimero", "/run/calimero"];
const MOCK_KMS_ATTESTATION_ENV: &str = "MEROD_ALLOW_MOCK_KMS_ATTESTATION";
const MAX_KMS_ATTEST_QUOTE_B64_LEN: usize = 128 * 1024;
const MAX_KMS_REPORT_DATA_HEX_LEN: usize = 1024;
const MAX_KMS_CHALLENGE_ID_LEN: usize = 512;
const MAX_KMS_NONCE_B64_LEN: usize = 2048;
const MAX_KMS_KEY_HEX_LEN: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KmsProbeStage {
    Transport,
    Attest,
    Challenge,
    GetKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KmsProbeResult {
    pub ok: bool,
    pub stage: KmsProbeStage,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kms_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

#[derive(Debug, Clone)]
struct ProbeAttestation {
    quote_bytes: Vec<u8>,
    quote_b64: String,
    is_mock: bool,
}

#[derive(Debug, Clone)]
struct KmsProbeFailure {
    stage: KmsProbeStage,
    code: &'static str,
    kms_error: Option<String>,
    details: String,
}

impl KmsProbeFailure {
    fn to_result(&self) -> KmsProbeResult {
        KmsProbeResult {
            ok: false,
            stage: self.stage,
            code: self.code.to_owned(),
            kms_error: self.kms_error.clone(),
            details: Some(self.details.clone()),
        }
    }
}

fn probe_failure(
    stage: KmsProbeStage,
    code: &'static str,
    kms_error: Option<String>,
    details: impl Into<String>,
) -> KmsProbeFailure {
    KmsProbeFailure {
        stage,
        code,
        kms_error,
        details: details.into(),
    }
}

fn probe_success(details: impl Into<String>) -> KmsProbeResult {
    KmsProbeResult {
        ok: true,
        stage: KmsProbeStage::GetKey,
        code: "OK".to_owned(),
        kms_error: None,
        details: Some(details.into()),
    }
}

fn map_kms_error_to_probe_code(kms_error: &str, fallback_code: &'static str) -> &'static str {
    match kms_error.trim().to_ascii_lowercase().as_str() {
        "measurement_policy_rejected" | "profile_policy_rejected" => "KMS_PROFILE_POLICY_REJECTED",
        _ => fallback_code,
    }
}

fn probe_failure_from_http(
    stage: KmsProbeStage,
    fallback_code: &'static str,
    error: &KmsHttpFailure,
) -> KmsProbeFailure {
    let code = error
        .kms_error
        .as_deref()
        .map(|kms_error| map_kms_error_to_probe_code(kms_error, fallback_code))
        .unwrap_or(fallback_code);

    probe_failure(stage, code, error.kms_error.clone(), error.to_string())
}

fn map_probe_attestation_failure(err: eyre::Report) -> KmsProbeFailure {
    if let Some(http_error) = err.downcast_ref::<KmsHttpFailure>() {
        return probe_failure_from_http(KmsProbeStage::Attest, "KMS_ATTEST_REJECTED", http_error);
    }

    let details = err.to_string();
    let code = if details.contains("reportData mismatch") {
        "KMS_ATTEST_REPORT_DATA_MISMATCH"
    } else if details.contains("mock attestation quote")
        && details.contains("accept_mock is disabled")
    {
        "KMS_ATTEST_MOCK_QUOTE_REJECTED"
    } else if details.contains(MOCK_KMS_ATTESTATION_ENV) {
        "KMS_ATTEST_MOCK_RUNTIME_NOT_ALLOWED"
    } else if details.contains("Policy JSON")
        || details.contains("allowed_")
        || details.contains("allowlist")
        || details.contains("TCB status")
        || details.contains("did not include TCB status")
    {
        "KMS_PROFILE_POLICY_REJECTED"
    } else if details.contains("exceeds maximum allowed size") {
        "KMS_ATTEST_RESPONSE_OVERSIZED"
    } else if details.contains("Failed to parse KMS attest response")
        || details.contains("Failed to decode KMS quote")
        || details.contains("Failed to decode reportDataHex")
        || details.contains("reportDataHex must be 64 bytes")
    {
        "KMS_ATTEST_RESPONSE_INVALID"
    } else {
        "KMS_ATTEST_VERIFICATION_FAILED"
    };

    probe_failure(KmsProbeStage::Attest, code, None, details)
}

fn map_probe_challenge_failure(err: eyre::Report) -> KmsProbeFailure {
    if let Some(http_error) = err.downcast_ref::<KmsHttpFailure>() {
        return probe_failure_from_http(
            KmsProbeStage::Challenge,
            "KMS_CHALLENGE_REJECTED",
            http_error,
        );
    }

    let details = err.to_string();
    let code = if details.contains("Failed to parse KMS challenge response") {
        "KMS_CHALLENGE_RESPONSE_INVALID"
    } else if details.contains("challengeId exceeds maximum allowed length") {
        "KMS_CHALLENGE_ID_OVERSIZED"
    } else if details.contains("challenge nonce exceeds maximum allowed size") {
        "KMS_CHALLENGE_NONCE_OVERSIZED"
    } else if details.contains("Failed to decode challenge nonce")
        || details.contains("Challenge nonce must be exactly 32 bytes")
    {
        "KMS_CHALLENGE_NONCE_INVALID"
    } else {
        "KMS_CHALLENGE_REQUEST_FAILED"
    };

    probe_failure(KmsProbeStage::Challenge, code, None, details)
}

fn map_probe_get_key_failure(err: eyre::Report) -> KmsProbeFailure {
    if let Some(http_error) = err.downcast_ref::<KmsHttpFailure>() {
        return probe_failure_from_http(KmsProbeStage::GetKey, "KMS_GET_KEY_REJECTED", http_error);
    }

    let details = err.to_string();
    let code = if details.contains("Failed to parse KMS get-key response") {
        "KMS_GET_KEY_RESPONSE_INVALID"
    } else if details.contains("empty encryption key")
        || details.contains("oversized encryption key")
        || details.contains("odd hex length")
        || details.contains("Failed to decode key from hex")
    {
        "KMS_KEY_INVALID"
    } else {
        "KMS_GET_KEY_REQUEST_FAILED"
    };

    probe_failure(KmsProbeStage::GetKey, code, None, details)
}

fn generate_probe_attestation(
    report_data: [u8; 64],
) -> std::result::Result<ProbeAttestation, String> {
    generate_attestation(report_data)
        .map(|attestation| ProbeAttestation {
            quote_bytes: attestation.quote_bytes,
            quote_b64: attestation.quote_b64,
            is_mock: attestation.is_mock,
        })
        .map_err(|err| format!("Failed to generate TDX attestation: {err}"))
}

pub async fn probe_storage_key(
    kms_config: &KmsConfig,
    peer_id: &str,
    identity: &Keypair,
) -> KmsProbeResult {
    let Some(phala_config) = kms_config.phala.as_ref() else {
        return probe_failure(
            KmsProbeStage::Transport,
            "KMS_PROVIDER_NOT_CONFIGURED",
            None,
            "TEE is enabled but tee.kms.phala is not configured",
        )
        .to_result();
    };

    match probe_phala_storage_key_with_attestor(
        phala_config,
        peer_id,
        identity,
        generate_probe_attestation,
    )
    .await
    {
        Ok(key_bytes) => probe_success(format!(
            "KMS probe succeeded and returned {} key bytes",
            key_bytes.len()
        )),
        Err(err) => err.to_result(),
    }
}

/// Fetch the storage encryption key using the configured KMS provider.
///
/// When `policy` is provided (from release-policy env vars), verifies the KMS
/// via POST /attest before requesting keys.
///
/// Returns an error if no KMS provider is configured (incomplete TEE configuration)
/// or if key fetching fails.
///
/// # Arguments
/// * `kms_config` - KMS configuration specifying which provider to use
/// * `peer_id` - The peer ID string (base58 encoded)
/// * `identity` - Local node identity keypair used to sign challenge payloads
/// * `policy` - Optional attestation policy fetched from a release version
pub async fn fetch_storage_key(
    kms_config: &KmsConfig,
    peer_id: &str,
    identity: &Keypair,
    policy: Option<&KmsAttestationPolicy>,
) -> Result<Vec<u8>> {
    if let Some(ref phala_config) = kms_config.phala {
        info!("Using Phala Cloud KMS");
        let strict_transport = policy.is_some()
            || (phala_config.attestation.enabled && !phala_config.attestation.accept_mock);
        validate_kms_transport_security(&phala_config.url, strict_transport)?;

        let attestation_mode = if let Some(p) = policy {
            verify_kms_attestation_from_release_policy(phala_config, p).await?;
            KeyFetchAttestationMode::AlreadyVerifiedFromReleasePolicy
        } else {
            KeyFetchAttestationMode::UseConfigPolicy
        };
        let key = fetch_from_phala(phala_config, peer_id, identity, attestation_mode).await?;
        Ok(key)
    } else {
        bail!(
            "TEE is enabled but no KMS provider is configured. \
             Please configure [tee.kms.phala] in your config.toml to enable storage encryption. \
             Running a TEE node without storage encryption is not supported."
        );
    }
}

async fn probe_phala_storage_key_with_attestor<F>(
    phala_config: &PhalaKmsConfig,
    peer_id: &str,
    identity: &Keypair,
    attestor: F,
) -> std::result::Result<Vec<u8>, KmsProbeFailure>
where
    F: Fn([u8; 64]) -> std::result::Result<ProbeAttestation, String>,
{
    let strict_transport =
        phala_config.attestation.enabled && !phala_config.attestation.accept_mock;
    validate_kms_transport_security(&phala_config.url, strict_transport).map_err(|err| {
        probe_failure(
            KmsProbeStage::Transport,
            "KMS_HTTP_INSECURE",
            None,
            err.to_string(),
        )
    })?;

    let base_url = ensure_trailing_slash(&phala_config.url);
    let challenge_endpoint = base_url.join("challenge").map_err(|err| {
        probe_failure(
            KmsProbeStage::Transport,
            "KMS_ENDPOINT_URL_INVALID",
            None,
            format!("Failed to build KMS challenge endpoint URL: {err}"),
        )
    })?;
    let key_endpoint = base_url.join("get-key").map_err(|err| {
        probe_failure(
            KmsProbeStage::Transport,
            "KMS_ENDPOINT_URL_INVALID",
            None,
            format!("Failed to build KMS get-key endpoint URL: {err}"),
        )
    })?;

    let client = build_kms_http_client(phala_config).map_err(|err| {
        probe_failure(
            KmsProbeStage::Transport,
            "KMS_HTTP_CLIENT_SETUP_FAILED",
            None,
            err.to_string(),
        )
    })?;

    if phala_config.attestation.enabled {
        verify_kms_attestation(&client, &base_url, &phala_config.attestation)
            .await
            .map_err(map_probe_attestation_failure)?;
    }

    let challenge = request_kms_challenge(&client, &challenge_endpoint, peer_id)
        .await
        .map_err(map_probe_challenge_failure)?;
    let challenge_nonce =
        decode_kms_challenge_nonce(&challenge).map_err(map_probe_challenge_failure)?;

    let peer_id_hash = hash_peer_id(peer_id);
    let mut report_data = [0u8; 64];
    report_data[..32].copy_from_slice(&challenge_nonce);
    report_data[32..].copy_from_slice(&peer_id_hash);

    let attestation = attestor(report_data).map_err(|details| {
        probe_failure(
            KmsProbeStage::GetKey,
            "KMS_GET_KEY_ATTESTATION_FAILED",
            None,
            details,
        )
    })?;
    if attestation.is_mock {
        warn!("Generated mock attestation during KMS probe");
    }

    let signature_payload = build_signature_payload(
        &challenge.challenge_id,
        &challenge_nonce,
        &attestation.quote_bytes,
        peer_id,
    )
    .map_err(|err| {
        probe_failure(
            KmsProbeStage::GetKey,
            "KMS_GET_KEY_SIGNATURE_FAILED",
            None,
            err.to_string(),
        )
    })?;
    let signature = identity.sign(&signature_payload).map_err(|err| {
        probe_failure(
            KmsProbeStage::GetKey,
            "KMS_GET_KEY_SIGNATURE_FAILED",
            None,
            format!("Failed to sign KMS challenge payload with node identity key: {err}"),
        )
    })?;
    let peer_public_key = identity.public().encode_protobuf();

    let key_response = request_kms_key_release(
        &client,
        &key_endpoint,
        &PhalaGetKeyRequest {
            challenge_id: challenge.challenge_id,
            quote_b64: attestation.quote_b64,
            peer_id: peer_id.to_owned(),
            peer_public_key_b64: base64::engine::general_purpose::STANDARD.encode(peer_public_key),
            signature_b64: base64::engine::general_purpose::STANDARD.encode(signature),
        },
    )
    .await
    .map_err(map_probe_get_key_failure)?;

    decode_kms_encryption_key(&key_response).map_err(map_probe_get_key_failure)
}

/// Verify KMS via POST /attest using policy fetched from release.
///
/// Calls KMS /attest, verifies the quote, and enforces measurement policy.
async fn verify_kms_attestation_from_release_policy(
    phala_config: &PhalaKmsConfig,
    policy: &KmsAttestationPolicy,
) -> Result<()> {
    info!("Verifying KMS attestation before key fetch");

    let base_url = ensure_trailing_slash(&phala_config.url);
    let attest_endpoint = base_url
        .join("attest")
        .context("Failed to build KMS attest endpoint URL")?;

    let client = build_kms_http_client(phala_config)?;

    let mut nonce = [0u8; 32];
    OsRng.fill_bytes(&mut nonce);
    let nonce_b64 = base64::engine::general_purpose::STANDARD.encode(nonce);

    let request = PhalaKmsAttestRequest {
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

    let attest: PhalaKmsAttestResponse = response
        .json()
        .await
        .context("Failed to parse KMS attest response")?;

    let binding_bytes = base64::engine::general_purpose::STANDARD
        .decode(&policy.default_binding_b64)
        .context("Invalid policy default_binding_b64")?;
    let binding: [u8; 32] = binding_bytes
        .try_into()
        .map_err(|_| eyre::eyre!("Policy default_binding_b64 must be 32 bytes"))?;
    let expected_report_data = build_kms_attestation_report_data(&nonce, &binding);

    let (quote_bytes, report_data_bytes) = decode_kms_attestation_response(&attest)?;
    if report_data_bytes.len() != 64 {
        bail!(
            "KMS attest reportDataHex must be 64 bytes, got {}",
            report_data_bytes.len()
        );
    }
    if !bool::from(report_data_bytes.ct_eq(expected_report_data.as_slice())) {
        bail!(
            "KMS attest reportData mismatch (nonce/binding mismatch or tampered response payload)"
        );
    }

    let is_mock = is_mock_quote(&quote_bytes);
    let verification_result = if is_mock {
        if !mock_kms_attestation_runtime_opt_in() {
            bail!(
                "KMS returned mock attestation quote, but {} is not enabled. \
                 Set {}=true only for development/testing.",
                MOCK_KMS_ATTESTATION_ENV,
                MOCK_KMS_ATTESTATION_ENV
            );
        }

        warn!(
            "Accepting mock KMS attestation quote from release policy path - development/testing only"
        );
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

    enforce_attestation_policy(policy, &verification_result, is_mock)?;
    info!("KMS attestation verified successfully");
    Ok(())
}

fn enforce_attestation_policy(
    policy: &KmsAttestationPolicy,
    verification_result: &calimero_tee_attestation::VerificationResult,
    allow_missing_tcb_status: bool,
) -> Result<()> {
    // Defense in depth: release-policy verification can run on paths that do
    // not rely on startup config validation, so we re-check required allowlists
    // here before enforcing measurement and TCB matching.
    enforce_required_allowlist_non_empty(
        "policy.allowed_tcb_statuses",
        &policy.allowed_tcb_statuses,
    )?;
    for (field_name, values) in [
        ("policy.allowed_mrtd", &policy.allowed_mrtd),
        ("policy.allowed_rtmr0", &policy.allowed_rtmr0),
        ("policy.allowed_rtmr1", &policy.allowed_rtmr1),
        ("policy.allowed_rtmr2", &policy.allowed_rtmr2),
        ("policy.allowed_rtmr3", &policy.allowed_rtmr3),
    ] {
        enforce_required_measurement_allowlist_non_empty(field_name, values)?;
    }

    if let Some(actual_tcb_status) = verification_result.tcb_status.as_ref() {
        let normalized_tcb = actual_tcb_status.to_ascii_lowercase();
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
    } else if allow_missing_tcb_status {
        warn!("Mock KMS quote did not provide TCB status; skipping TCB status allowlist check");
    } else {
        bail!("Quote verification did not provide a TCB status");
    }

    let body = &verification_result.quote.body;
    enforce_measurement_allowlist_for_release_policy("MRTD", &body.mrtd, &policy.allowed_mrtd)?;
    enforce_measurement_allowlist_for_release_policy("RTMR0", &body.rtmr0, &policy.allowed_rtmr0)?;
    enforce_measurement_allowlist_for_release_policy("RTMR1", &body.rtmr1, &policy.allowed_rtmr1)?;
    enforce_measurement_allowlist_for_release_policy("RTMR2", &body.rtmr2, &policy.allowed_rtmr2)?;
    enforce_measurement_allowlist_for_release_policy("RTMR3", &body.rtmr3, &policy.allowed_rtmr3)?;
    Ok(())
}

fn enforce_measurement_allowlist_for_release_policy(
    label: &str,
    actual: &str,
    allowed: &[String],
) -> Result<()> {
    let normalized = normalize_attestation_measurement(actual);
    if allowed.iter().any(|a| a == &normalized) {
        return Ok(());
    }

    let preview_len = 5usize.min(allowed.len());
    let preview = allowed[..preview_len].join(", ");
    let suffix = if allowed.len() > preview_len {
        format!(" ... ({} total)", allowed.len())
    } else {
        String::new()
    };

    bail!(
        "KMS {} '{}' is not in allowlist [{}{}]",
        label,
        normalized,
        preview,
        suffix
    )
}

fn enforce_required_allowlist_non_empty(field_name: &str, values: &[String]) -> Result<()> {
    if values.iter().all(|value| value.trim().is_empty()) {
        bail!(
            "KMS attestation policy is missing required non-empty allowlist '{}'",
            field_name
        );
    }
    Ok(())
}

fn enforce_required_measurement_allowlist_non_empty(
    field_name: &str,
    values: &[String],
) -> Result<()> {
    if values
        .iter()
        .map(|value| normalize_attestation_measurement(value))
        .all(|value| value.is_empty())
    {
        bail!(
            "KMS attestation policy is missing required non-empty allowlist '{}'",
            field_name
        );
    }
    Ok(())
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
    attestation_mode: KeyFetchAttestationMode,
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

    // Build HTTP client once and reuse for all KMS requests.
    let client = build_kms_http_client(phala_config)?;

    if phala_config.attestation.enabled {
        match attestation_mode {
            KeyFetchAttestationMode::UseConfigPolicy => {
                verify_kms_attestation(&client, &base_url, &phala_config.attestation).await?;
            }
            KeyFetchAttestationMode::AlreadyVerifiedFromReleasePolicy => {
                info!(
                    "Skipping config-based KMS attestation: release policy verification already completed"
                );
            }
        }
    }

    // 1) Request one-time challenge nonce.
    info!(%challenge_endpoint, "Requesting key release challenge from KMS");
    let challenge = request_kms_challenge(&client, &challenge_endpoint, peer_id).await?;
    let challenge_nonce = decode_kms_challenge_nonce(&challenge)?;

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
    let response = request_kms_key_release(&client, &key_endpoint, &request).await?;
    let key_bytes = decode_kms_encryption_key(&response)?;

    info!(
        key_len = key_bytes.len(),
        "Successfully fetched storage key from KMS"
    );

    Ok(key_bytes)
}

fn build_kms_http_client(phala_config: &PhalaKmsConfig) -> Result<reqwest::Client> {
    let uses_https = phala_config.url.scheme().eq_ignore_ascii_case("https");
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30));

    // Keep TLS invariants here even though startup config validation checks the
    // same constraints. This preserves fail-closed behavior if config is edited
    // between validation and client construction.
    if let Some(ca_cert_path) = phala_config.tls.ca_cert_path.as_deref() {
        if !uses_https {
            bail!(
                "tee.kms.phala.tls.ca_cert_path requires tee.kms.phala.url to use https:// (current: {})",
                phala_config.url
            );
        }
        // Intentional sync I/O: this path is executed during startup preflight
        // key-fetch/probe setup and keeps client construction fail-closed.
        let ca_pem = std::fs::read(ca_cert_path).with_context(|| {
            format!(
                "Failed to read tee.kms.phala.tls.ca_cert_path at {}",
                ca_cert_path
            )
        })?;
        let cert = reqwest::Certificate::from_pem(&ca_pem)
            .context("Failed to parse tee.kms.phala.tls.ca_cert_path as PEM certificate")?;
        builder = builder.add_root_certificate(cert);
    }

    match (
        phala_config.tls.client_cert_path.as_deref(),
        phala_config.tls.client_key_path.as_deref(),
    ) {
        (Some(client_cert_path), Some(client_key_path)) => {
            if !uses_https {
                bail!(
                    "tee.kms.phala.tls.client_cert_path/client_key_path require tee.kms.phala.url to use https:// (current: {})",
                    phala_config.url
                );
            }

            // Intentional sync I/O for startup-only TLS material loading.
            let client_cert_pem = std::fs::read(client_cert_path).with_context(|| {
                format!(
                    "Failed to read tee.kms.phala.tls.client_cert_path at {}",
                    client_cert_path
                )
            })?;
            let client_key_pem = std::fs::read(client_key_path).with_context(|| {
                format!(
                    "Failed to read tee.kms.phala.tls.client_key_path at {}",
                    client_key_path
                )
            })?;

            let mut identity_pem =
                Vec::with_capacity(client_cert_pem.len() + client_key_pem.len() + 1);
            identity_pem.extend_from_slice(&client_cert_pem);
            if !identity_pem.ends_with(b"\n") {
                identity_pem.push(b'\n');
            }
            identity_pem.extend_from_slice(&client_key_pem);

            let identity = reqwest::Identity::from_pem(&identity_pem).context(
                "Failed to parse tee.kms.phala.tls.client_cert_path/client_key_path as PEM identity",
            )?;
            builder = builder.identity(identity);
        }
        (None, None) => {}
        _ => {
            bail!(
                "tee.kms.phala.tls.client_cert_path and tee.kms.phala.tls.client_key_path must be set together"
            );
        }
    }

    builder.build().context("Failed to build HTTP client")
}

async fn verify_kms_attestation(
    client: &reqwest::Client,
    base_url: &Url,
    attestation_config: &KmsAttestationConfig,
) -> Result<()> {
    let effective_config = resolve_effective_attestation_config(attestation_config)?;
    let policy = normalize_kms_attestation_policy(&effective_config)?;
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

    if !bool::from(report_data_bytes.ct_eq(expected_report_data.as_slice())) {
        bail!(
            "KMS attest reportData mismatch (nonce/binding mismatch or tampered response payload)"
        );
    }

    let verification_result = if is_mock_quote(&quote_bytes) {
        if !policy.accept_mock {
            bail!("KMS returned mock attestation quote, but attestation.accept_mock is disabled");
        }
        if !mock_kms_attestation_runtime_opt_in() {
            bail!(
                "KMS returned mock attestation quote, but {} is not enabled. \
                 Set {}=true only for development/testing.",
                MOCK_KMS_ATTESTATION_ENV,
                MOCK_KMS_ATTESTATION_ENV
            );
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

pub(crate) fn resolve_effective_attestation_config(
    config: &KmsAttestationConfig,
) -> Result<KmsAttestationConfig> {
    let mut effective_config = config.clone();

    if !config.enabled {
        return Ok(effective_config);
    }

    if let Some(policy_path) = config.policy_json_path.as_deref() {
        if !policy_path.is_absolute() {
            bail!(
                "tee.kms.phala.attestation.policy_json_path must be an absolute path: {}",
                policy_path
            );
        }

        let policy_path = canonicalize_external_policy_path(policy_path)?;
        if !is_allowed_external_policy_path(&policy_path) {
            bail!(
                "tee.kms.phala.attestation.policy_json_path must be under one of: {}",
                EXTERNAL_POLICY_ALLOWED_DIRS.join(", ")
            );
        }

        let external_policy = load_external_attestation_policy(&policy_path)?;
        let nested_policy = external_policy
            .policy
            .unwrap_or(ExternalKmsAttestationPolicyValues {
                allowed_tcb_statuses: None,
                allowed_mrtd: None,
                allowed_rtmr0: None,
                allowed_rtmr1: None,
                allowed_rtmr2: None,
                allowed_rtmr3: None,
            });
        let nested_kms = external_policy
            .kms
            .unwrap_or(ExternalKmsAttestationPolicyKms {
                default_binding_b64: None,
            });

        // Explicit `Some([])` from external policy intentionally clears the
        // base allowlist (rather than "unset"), then validation enforces
        // required production fields such as allowed_mrtd and allowed_rtmr0..3.
        merge_external_allowlist(
            &mut effective_config.allowed_tcb_statuses,
            external_policy.allowed_tcb_statuses,
            nested_policy.allowed_tcb_statuses,
        );
        merge_external_allowlist(
            &mut effective_config.allowed_mrtd,
            external_policy.allowed_mrtd,
            nested_policy.allowed_mrtd,
        );
        merge_external_allowlist(
            &mut effective_config.allowed_rtmr0,
            external_policy.allowed_rtmr0,
            nested_policy.allowed_rtmr0,
        );
        merge_external_allowlist(
            &mut effective_config.allowed_rtmr1,
            external_policy.allowed_rtmr1,
            nested_policy.allowed_rtmr1,
        );
        merge_external_allowlist(
            &mut effective_config.allowed_rtmr2,
            external_policy.allowed_rtmr2,
            nested_policy.allowed_rtmr2,
        );
        merge_external_allowlist(
            &mut effective_config.allowed_rtmr3,
            external_policy.allowed_rtmr3,
            nested_policy.allowed_rtmr3,
        );
        if let Some(value) = external_policy
            .binding_b64
            .or(nested_kms.default_binding_b64)
        {
            effective_config.binding_b64 = Some(value);
        }
        // Mark policy as already resolved to avoid re-reading the JSON file on
        // subsequent startup preflight calls.
        effective_config.policy_json_path = None;

        info!(
            policy_path = %policy_path,
            "Loaded external KMS attestation policy"
        );
    }

    effective_config.validate_enabled_policy()?;
    Ok(effective_config)
}

fn merge_external_allowlist(
    target: &mut Vec<String>,
    flat_override: Option<Vec<String>>,
    nested_override: Option<Vec<String>>,
) {
    if let Some(values) = flat_override.or(nested_override) {
        *target = values;
    }
}

fn load_external_attestation_policy(
    policy_path: &Utf8Path,
) -> Result<ExternalKmsAttestationPolicy> {
    // Intentional sync I/O: this path is used during startup preflight only.
    let policy_raw = std::fs::read_to_string(policy_path).with_context(|| {
        format!(
            "Failed to read external KMS attestation policy file at {}",
            policy_path
        )
    })?;

    serde_json::from_str::<ExternalKmsAttestationPolicy>(&policy_raw).with_context(|| {
        format!(
            "Failed to parse external KMS attestation policy JSON at {}",
            policy_path
        )
    })
}

fn canonicalize_external_policy_path(policy_path: &Utf8Path) -> Result<Utf8PathBuf> {
    let canonical_path = std::fs::canonicalize(policy_path).with_context(|| {
        format!(
            "Failed to canonicalize external KMS attestation policy path at {}",
            policy_path
        )
    })?;
    Utf8PathBuf::from_path_buf(canonical_path).map_err(|path| {
        eyre::eyre!(
            "External KMS attestation policy path is not valid UTF-8: {}",
            path.display()
        )
    })
}

fn is_allowed_external_policy_path(policy_path: &Utf8Path) -> bool {
    EXTERNAL_POLICY_ALLOWED_DIRS
        .iter()
        .any(|allowed_dir| policy_path.starts_with(allowed_dir))
        || is_test_tmp_policy_path(policy_path)
}

fn is_test_tmp_policy_path(policy_path: &Utf8Path) -> bool {
    // Allow /tmp and /private/tmp (macOS canonicalizes /tmp to /private/tmp) in tests
    // so tempfile-backed policy fixtures work. Test binaries must never be deployed to production.
    cfg!(test) && (policy_path.starts_with("/tmp") || policy_path.starts_with("/private/tmp"))
}

async fn request_kms_challenge(
    client: &reqwest::Client,
    challenge_endpoint: &Url,
    peer_id: &str,
) -> Result<PhalaChallengeResponse> {
    let challenge_response = client
        .post(challenge_endpoint.as_str())
        .json(&PhalaChallengeRequest {
            peer_id: peer_id.to_owned(),
        })
        .send()
        .await
        .context("Failed to request challenge from KMS")?;

    let challenge_status = challenge_response.status();
    if !challenge_status.is_success() {
        let error_body = challenge_response.text().await.unwrap_or_default();
        return Err(
            KmsHttpFailure::from_response("challenge", challenge_status, &error_body).into(),
        );
    }

    challenge_response
        .json()
        .await
        .context("Failed to parse KMS challenge response")
}

fn decode_kms_challenge_nonce(challenge: &PhalaChallengeResponse) -> Result<[u8; 32]> {
    if challenge.challenge_id.len() > MAX_KMS_CHALLENGE_ID_LEN {
        bail!(
            "KMS challengeId exceeds maximum allowed length ({} chars)",
            MAX_KMS_CHALLENGE_ID_LEN
        );
    }
    if challenge.nonce_b64.len() > MAX_KMS_NONCE_B64_LEN {
        bail!(
            "KMS challenge nonce exceeds maximum allowed size ({} chars base64)",
            MAX_KMS_NONCE_B64_LEN
        );
    }
    let challenge_nonce_vec = base64::engine::general_purpose::STANDARD
        .decode(&challenge.nonce_b64)
        .context("Failed to decode challenge nonce from base64")?;
    challenge_nonce_vec
        .try_into()
        .map_err(|_| eyre::eyre!("Challenge nonce must be exactly 32 bytes"))
}

async fn request_kms_key_release(
    client: &reqwest::Client,
    key_endpoint: &Url,
    request: &PhalaGetKeyRequest,
) -> Result<PhalaGetKeyResponse> {
    let response = client
        .post(key_endpoint.as_str())
        .json(request)
        .send()
        .await
        .context("Failed to send request to KMS")?;

    let status = response.status();
    if !status.is_success() {
        let error_body = response.text().await.unwrap_or_default();
        return Err(KmsHttpFailure::from_response("get-key", status, &error_body).into());
    }

    response
        .json()
        .await
        .context("Failed to parse KMS get-key response")
}

fn decode_kms_encryption_key(response: &PhalaGetKeyResponse) -> Result<Vec<u8>> {
    let key_hex = response.key.trim();
    if key_hex.is_empty() {
        bail!("KMS returned an empty encryption key");
    }
    if key_hex.len() > MAX_KMS_KEY_HEX_LEN {
        bail!(
            "KMS returned oversized encryption key ({} hex chars > {})",
            key_hex.len(),
            MAX_KMS_KEY_HEX_LEN
        );
    }
    if key_hex.len() % 2 != 0 {
        bail!("KMS returned encryption key with invalid odd hex length");
    }

    hex::decode(key_hex).context("Failed to decode key from hex")
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
        return Err(KmsHttpFailure::from_response("attestation", status, &error_body).into());
    }

    response
        .json()
        .await
        .context("Failed to parse KMS attest response")
}

fn decode_kms_attestation_response(
    attest_response: &PhalaKmsAttestResponse,
) -> Result<(Vec<u8>, Vec<u8>)> {
    if attest_response.quote_b64.is_empty() {
        bail!("KMS attest response quoteB64 is empty");
    }
    if attest_response.quote_b64.len() > MAX_KMS_ATTEST_QUOTE_B64_LEN {
        bail!(
            "KMS attest response quoteB64 exceeds maximum allowed size ({} chars > {})",
            attest_response.quote_b64.len(),
            MAX_KMS_ATTEST_QUOTE_B64_LEN
        );
    }
    if attest_response.report_data_hex.is_empty() {
        bail!("KMS attest response reportDataHex is empty");
    }
    if attest_response.report_data_hex.len() > MAX_KMS_REPORT_DATA_HEX_LEN {
        bail!(
            "KMS attest response reportDataHex exceeds maximum allowed size ({} chars > {})",
            attest_response.report_data_hex.len(),
            MAX_KMS_REPORT_DATA_HEX_LEN
        );
    }

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
        let normalized = normalize_attestation_measurement(raw);
        if normalized.is_empty() {
            continue;
        }

        if normalized.len() != 96 {
            bail!(
                "{field_name} contains invalid measurement length (expected 48 bytes for TDX measurement, got {} bytes)",
                normalized.len() / 2
            );
        }

        hex::decode(&normalized)
            .with_context(|| format!("{field_name} contains non-hex value: {raw}"))?;

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
    let strict_policy = !policy.accept_mock;
    if strict_policy {
        enforce_required_allowlist_non_empty(
            "tee.kms.phala.attestation.allowed_tcb_statuses",
            &policy.allowed_tcb_statuses,
        )?;
        for (field_name, values) in [
            (
                "tee.kms.phala.attestation.allowed_mrtd",
                &policy.allowed_mrtd,
            ),
            (
                "tee.kms.phala.attestation.allowed_rtmr0",
                &policy.allowed_rtmr0,
            ),
            (
                "tee.kms.phala.attestation.allowed_rtmr1",
                &policy.allowed_rtmr1,
            ),
            (
                "tee.kms.phala.attestation.allowed_rtmr2",
                &policy.allowed_rtmr2,
            ),
            (
                "tee.kms.phala.attestation.allowed_rtmr3",
                &policy.allowed_rtmr3,
            ),
        ] {
            enforce_required_measurement_allowlist_non_empty(field_name, values)?;
        }
    }

    if !strict_policy && policy.allowed_tcb_statuses.is_empty() {
        debug!("Skipping TCB status allowlist check (allowlist is empty in mock mode)");
    } else {
        let actual_tcb_status = verification_result
            .tcb_status
            .as_deref()
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
    }

    let body = &verification_result.quote.body;
    enforce_measurement_allowlist("MRTD", &body.mrtd, &policy.allowed_mrtd)?;
    enforce_measurement_allowlist("RTMR0", &body.rtmr0, &policy.allowed_rtmr0)?;
    enforce_measurement_allowlist("RTMR1", &body.rtmr1, &policy.allowed_rtmr1)?;
    enforce_measurement_allowlist("RTMR2", &body.rtmr2, &policy.allowed_rtmr2)?;
    enforce_measurement_allowlist("RTMR3", &body.rtmr3, &policy.allowed_rtmr3)?;

    Ok(())
}

fn mock_kms_attestation_runtime_opt_in() -> bool {
    std::env::var(MOCK_KMS_ATTESTATION_ENV)
        .map(|value| value.eq_ignore_ascii_case("true") || value == "1")
        .unwrap_or(false)
}

fn enforce_measurement_allowlist(
    label: &str,
    actual_measurement: &str,
    allowed_measurements: &[String],
) -> Result<()> {
    if allowed_measurements.is_empty() {
        debug!(
            measurement = label,
            "Skipping measurement allowlist check (allowlist is empty)"
        );
        return Ok(());
    }

    let normalized_actual = normalize_attestation_measurement(actual_measurement);
    if allowed_measurements
        .iter()
        .any(|allowed| allowed == &normalized_actual)
    {
        return Ok(());
    }

    bail!("{label} '{}' is not in allowlist", normalized_actual);
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

fn validate_kms_transport_security(kms_url: &Url, strict_mode: bool) -> Result<()> {
    match kms_url.scheme() {
        "https" => Ok(()),
        "http" => {
            if is_loopback_kms_host(kms_url) {
                return Ok(());
            }
            if strict_mode {
                bail!(
                    "In production attestation mode, tee.kms.phala.url must use HTTPS or loopback HTTP to prevent KMS spoofing: {}",
                    kms_url
                );
            }
            warn!(
                kms_url = %kms_url,
                "Using non-loopback HTTP KMS endpoint; this is susceptible to spoofing/MITM and should only be used in trusted development setups"
            );
            Ok(())
        }
        scheme => bail!(
            "Unsupported KMS URL scheme '{}'; expected https:// (or http:// for loopback development only)",
            scheme
        ),
    }
}

fn is_loopback_kms_host(kms_url: &Url) -> bool {
    match kms_url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::{Json, Router};
    use camino::Utf8PathBuf;
    use serde::Deserialize;
    use serde_json::json;
    use tempfile::NamedTempFile;

    #[derive(Clone, Copy)]
    enum AttestResponseMode {
        Valid,
        ReportDataMismatch,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct AttestRequestBody {
        nonce_b64: String,
        #[serde(default)]
        binding_b64: Option<String>,
    }

    fn mock_quote_bytes_with_report_data(report_data: &[u8; 64]) -> Vec<u8> {
        let mut quote_bytes = b"MOCK_TDX_QUOTE_V1".to_vec();
        quote_bytes.extend_from_slice(report_data);
        quote_bytes.resize(256, 0);
        quote_bytes
    }

    async fn attest_handler(
        State(mode): State<AttestResponseMode>,
        Json(request): Json<AttestRequestBody>,
    ) -> Json<serde_json::Value> {
        let nonce_bytes = base64::engine::general_purpose::STANDARD
            .decode(&request.nonce_b64)
            .expect("request nonce must be valid base64");
        let nonce: [u8; 32] = nonce_bytes
            .try_into()
            .expect("request nonce must decode to 32 bytes");

        let binding = if let Some(binding_b64) = request.binding_b64 {
            let binding_bytes = base64::engine::general_purpose::STANDARD
                .decode(binding_b64)
                .expect("binding must be valid base64");
            binding_bytes
                .try_into()
                .expect("binding must decode to 32 bytes")
        } else {
            default_kms_attestation_binding()
        };

        let expected_report_data = build_kms_attestation_report_data(&nonce, &binding);
        let report_data_hex = match mode {
            AttestResponseMode::Valid => hex::encode(expected_report_data),
            AttestResponseMode::ReportDataMismatch => hex::encode([0u8; 64]),
        };

        let quote_b64 = base64::engine::general_purpose::STANDARD
            .encode(mock_quote_bytes_with_report_data(&expected_report_data));

        Json(json!({
            "quoteB64": quote_b64,
            "reportDataHex": report_data_hex
        }))
    }

    async fn spawn_attest_server(mode: AttestResponseMode) -> Url {
        let app = Router::new()
            .route("/attest", post(attest_handler))
            .with_state(mode);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener
            .local_addr()
            .expect("listener should have local addr");

        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("attest test server should run");
        });

        Url::parse(&format!("http://{addr}/")).expect("base URL should parse")
    }

    #[derive(Clone, Copy)]
    enum ProbeChallengeMode {
        Valid,
        MalformedNonce,
        OversizedNonce,
    }

    #[derive(Clone, Copy)]
    enum ProbeGetKeyMode {
        Success,
        MeasurementPolicyRejected,
    }

    #[derive(Clone, Copy)]
    struct ProbeServerMode {
        attest: AttestResponseMode,
        challenge: ProbeChallengeMode,
        get_key: ProbeGetKeyMode,
    }

    #[derive(Clone)]
    struct ProbeServerState {
        mode: ProbeServerMode,
        get_key_hits: Arc<AtomicUsize>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ChallengeRequestBody {
        peer_id: String,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GetKeyRequestBody {
        challenge_id: String,
        quote_b64: String,
        peer_id: String,
        peer_public_key_b64: String,
        signature_b64: String,
    }

    async fn probe_attest_handler(
        State(state): State<ProbeServerState>,
        Json(request): Json<AttestRequestBody>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        let nonce_bytes = base64::engine::general_purpose::STANDARD
            .decode(&request.nonce_b64)
            .expect("request nonce must be valid base64");
        let nonce: [u8; 32] = nonce_bytes
            .try_into()
            .expect("request nonce must decode to 32 bytes");

        let binding = if let Some(binding_b64) = request.binding_b64 {
            let binding_bytes = base64::engine::general_purpose::STANDARD
                .decode(binding_b64)
                .expect("binding must be valid base64");
            binding_bytes
                .try_into()
                .expect("binding must decode to 32 bytes")
        } else {
            default_kms_attestation_binding()
        };

        let expected_report_data = build_kms_attestation_report_data(&nonce, &binding);
        let report_data_hex = match state.mode.attest {
            AttestResponseMode::Valid => hex::encode(expected_report_data),
            AttestResponseMode::ReportDataMismatch => hex::encode([0u8; 64]),
        };

        let quote_b64 = base64::engine::general_purpose::STANDARD
            .encode(mock_quote_bytes_with_report_data(&expected_report_data));

        (
            StatusCode::OK,
            Json(json!({
                "quoteB64": quote_b64,
                "reportDataHex": report_data_hex
            })),
        )
    }

    async fn probe_challenge_handler(
        State(state): State<ProbeServerState>,
        Json(request): Json<ChallengeRequestBody>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        let _ = request.peer_id;

        let nonce_b64 = match state.mode.challenge {
            ProbeChallengeMode::Valid => {
                base64::engine::general_purpose::STANDARD.encode([0x33u8; 32])
            }
            ProbeChallengeMode::MalformedNonce => "***not-base64***".to_owned(),
            ProbeChallengeMode::OversizedNonce => "A".repeat(MAX_KMS_NONCE_B64_LEN + 1),
        };

        (
            StatusCode::OK,
            Json(json!({
                "challengeId": "probe-challenge",
                "nonceB64": nonce_b64
            })),
        )
    }

    async fn probe_get_key_handler(
        State(state): State<ProbeServerState>,
        Json(request): Json<GetKeyRequestBody>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        state.get_key_hits.fetch_add(1, Ordering::SeqCst);

        let has_empty_field = request.challenge_id.is_empty()
            || request.quote_b64.is_empty()
            || request.peer_id.is_empty()
            || request.peer_public_key_b64.is_empty()
            || request.signature_b64.is_empty();
        if has_empty_field {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "invalid_request",
                    "details": "required request fields are missing"
                })),
            );
        }

        match state.mode.get_key {
            ProbeGetKeyMode::Success => (
                StatusCode::OK,
                Json(json!({
                    "key": "11".repeat(32)
                })),
            ),
            ProbeGetKeyMode::MeasurementPolicyRejected => (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "error": "measurement_policy_rejected",
                    "details": "MRTD mismatch"
                })),
            ),
        }
    }

    async fn spawn_probe_server(mode: ProbeServerMode) -> (Url, Arc<AtomicUsize>) {
        let get_key_hits = Arc::new(AtomicUsize::new(0));
        let state = ProbeServerState {
            mode,
            get_key_hits: Arc::clone(&get_key_hits),
        };

        let app = Router::new()
            .route("/attest", post(probe_attest_handler))
            .route("/challenge", post(probe_challenge_handler))
            .route("/get-key", post(probe_get_key_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener
            .local_addr()
            .expect("listener should have local addr");

        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("probe test server should run");
        });

        (
            Url::parse(&format!("http://{addr}/")).expect("base URL should parse"),
            get_key_hits,
        )
    }

    fn make_probe_phala_config(url: &Url, attestation_enabled: bool) -> PhalaKmsConfig {
        if attestation_enabled {
            return parse_phala_config(json!({
                "url": url.as_str(),
                "attestation": {
                    "enabled": true,
                    "accept_mock": true,
                    "allowed_tcb_statuses": ["Mock"],
                    "allowed_mrtd": ["00".repeat(48)],
                    "allowed_rtmr0": ["00".repeat(48)],
                    "allowed_rtmr1": ["00".repeat(48)],
                    "allowed_rtmr2": ["00".repeat(48)],
                    "allowed_rtmr3": ["00".repeat(48)]
                }
            }));
        }

        parse_phala_config(json!({
            "url": url.as_str(),
            "attestation": {
                "enabled": false
            }
        }))
    }

    fn mock_probe_attestor(report_data: [u8; 64]) -> std::result::Result<ProbeAttestation, String> {
        let quote_bytes = mock_quote_bytes_with_report_data(&report_data);
        let quote_b64 = base64::engine::general_purpose::STANDARD.encode(&quote_bytes);
        Ok(ProbeAttestation {
            quote_bytes,
            quote_b64,
            is_mock: true,
        })
    }

    fn write_temp_policy_file(contents: &str) -> NamedTempFile {
        let mut file = tempfile::Builder::new()
            .prefix("merod-kms-policy-")
            .suffix(".json")
            .tempfile_in("/tmp")
            .expect("temp policy file should be created");
        file.write_all(contents.as_bytes())
            .expect("should write temp policy file");
        file
    }

    fn enable_mock_kms_attestation_env() {
        // Tests that exercise mock quote acceptance must explicitly opt in to
        // mirror production behavior.
        unsafe {
            std::env::set_var(MOCK_KMS_ATTESTATION_ENV, "true");
        }
    }

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
    fn test_validate_kms_transport_security_allows_https_and_loopback_http() {
        let https_url = Url::parse("https://kms.example.com/").unwrap();
        assert!(validate_kms_transport_security(&https_url, true).is_ok());

        let loopback_http = Url::parse("http://127.0.0.1:8080/").unwrap();
        assert!(validate_kms_transport_security(&loopback_http, true).is_ok());

        let loopback_http_v6 = Url::parse("http://[::1]:8080/").unwrap();
        assert!(validate_kms_transport_security(&loopback_http_v6, true).is_ok());
    }

    #[test]
    fn test_validate_kms_transport_security_rejects_non_loopback_http_in_strict_mode() {
        let remote_http = Url::parse("http://kms.example.com/").unwrap();
        let err = validate_kms_transport_security(&remote_http, true)
            .expect_err("strict mode must reject non-loopback HTTP")
            .to_string();
        assert!(err.contains("must use HTTPS or loopback HTTP"));

        let localhost_http = Url::parse("http://localhost:8080/").unwrap();
        let err = validate_kms_transport_security(&localhost_http, true)
            .expect_err("strict mode must reject hostname-based localhost")
            .to_string();
        assert!(err.contains("must use HTTPS or loopback HTTP"));
    }

    fn parse_phala_config(value: serde_json::Value) -> PhalaKmsConfig {
        serde_json::from_value(value).expect("valid phala config fixture")
    }

    #[test]
    fn test_build_kms_http_client_rejects_partial_mtls_configuration() {
        let cfg = parse_phala_config(json!({
            "url": "https://kms.example.com/",
            "tls": {
                "client_cert_path": "/etc/calimero/client-cert.pem"
            }
        }));
        let err = build_kms_http_client(&cfg)
            .expect_err("partial mTLS config must fail")
            .to_string();
        assert!(err.contains("must be set together"));
    }

    #[test]
    fn test_build_kms_http_client_rejects_ca_pinning_on_http() {
        let cfg = parse_phala_config(json!({
            "url": "http://127.0.0.1:8080/",
            "tls": {
                "ca_cert_path": "/etc/calimero/kms-ca.pem"
            }
        }));
        let err = build_kms_http_client(&cfg)
            .expect_err("CA pinning over HTTP must fail")
            .to_string();
        assert!(err.contains("requires tee.kms.phala.url to use https://"));
    }

    #[test]
    fn test_decode_kms_attestation_response_rejects_oversized_fields() {
        let oversized_quote = PhalaKmsAttestResponse {
            quote_b64: "A".repeat(MAX_KMS_ATTEST_QUOTE_B64_LEN + 1),
            report_data_hex: "00".repeat(64),
        };
        let err = decode_kms_attestation_response(&oversized_quote)
            .expect_err("oversized quoteB64 must fail")
            .to_string();
        assert!(err.contains("quoteB64 exceeds maximum allowed size"));

        let oversized_report = PhalaKmsAttestResponse {
            quote_b64: base64::engine::general_purpose::STANDARD.encode([0u8; 32]),
            report_data_hex: "0".repeat(MAX_KMS_REPORT_DATA_HEX_LEN + 1),
        };
        let err = decode_kms_attestation_response(&oversized_report)
            .expect_err("oversized reportDataHex must fail")
            .to_string();
        assert!(err.contains("reportDataHex exceeds maximum allowed size"));
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
    fn test_parse_measurement_allowlist_accepts_uppercase_prefixed_hex() {
        let values = vec![format!("0X{}", "AB".repeat(48))];
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
        let result = resolve_effective_attestation_config(&cfg);
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

    #[test]
    fn test_resolve_effective_attestation_config_applies_external_policy() {
        let policy_file = write_temp_policy_file(&format!(
            r#"{{
  "allowed_tcb_statuses": ["Mock"],
  "allowed_mrtd": ["{measurement}"],
  "allowed_rtmr0": ["{measurement}"],
  "allowed_rtmr1": ["{measurement}"],
  "allowed_rtmr2": ["{measurement}"],
  "allowed_rtmr3": ["{measurement}"]
}}"#,
            measurement = "00".repeat(48)
        ));
        let policy_path = Utf8PathBuf::from_path_buf(policy_file.path().to_path_buf())
            .expect("temp policy path should be valid utf-8");

        let mut cfg = KmsAttestationConfig::default();
        cfg.enabled = true;
        cfg.allowed_tcb_statuses = vec!["UpToDate".to_owned()];
        cfg.allowed_mrtd = vec!["ab".repeat(48)];
        cfg.policy_json_path = Some(policy_path.clone());

        let resolved = resolve_effective_attestation_config(&cfg).unwrap();
        assert_eq!(resolved.allowed_tcb_statuses, vec!["Mock".to_owned()]);
        assert_eq!(resolved.allowed_mrtd, vec!["00".repeat(48)]);
        assert_eq!(resolved.allowed_rtmr0, vec!["00".repeat(48)]);
        assert_eq!(resolved.allowed_rtmr1, vec!["00".repeat(48)]);
        assert_eq!(resolved.allowed_rtmr2, vec!["00".repeat(48)]);
        assert_eq!(resolved.allowed_rtmr3, vec!["00".repeat(48)]);
    }

    #[test]
    fn test_resolve_effective_attestation_config_accepts_mero_tee_policy_shape() {
        let binding_b64 = base64::engine::general_purpose::STANDARD.encode([0x33u8; 32]);
        let policy_file = write_temp_policy_file(&format!(
            r#"{{
  "schema_version": 2,
  "policy": {{
    "allowed_tcb_statuses": ["Mock"],
    "allowed_mrtd": ["{mrtd}"],
    "allowed_rtmr0": ["{rtmr0}"],
    "allowed_rtmr1": ["{rtmr1}"],
    "allowed_rtmr2": ["{rtmr2}"],
    "allowed_rtmr3": ["{rtmr3}"]
  }},
  "kms": {{
    "default_binding_b64": "{binding}"
  }}
}}"#,
            mrtd = "00".repeat(48),
            rtmr0 = "11".repeat(48),
            rtmr1 = "22".repeat(48),
            rtmr2 = "33".repeat(48),
            rtmr3 = "44".repeat(48),
            binding = binding_b64
        ));
        let policy_path = Utf8PathBuf::from_path_buf(policy_file.path().to_path_buf())
            .expect("temp policy path should be valid utf-8");

        let mut cfg = KmsAttestationConfig::default();
        cfg.enabled = true;
        cfg.allowed_tcb_statuses = vec!["UpToDate".to_owned()];
        cfg.allowed_mrtd = vec!["ab".repeat(48)];
        cfg.policy_json_path = Some(policy_path);

        let resolved = resolve_effective_attestation_config(&cfg).unwrap();
        assert_eq!(resolved.allowed_tcb_statuses, vec!["Mock".to_owned()]);
        assert_eq!(resolved.allowed_mrtd, vec!["00".repeat(48)]);
        assert_eq!(resolved.allowed_rtmr0, vec!["11".repeat(48)]);
        assert_eq!(resolved.allowed_rtmr1, vec!["22".repeat(48)]);
        assert_eq!(resolved.allowed_rtmr2, vec!["33".repeat(48)]);
        assert_eq!(resolved.allowed_rtmr3, vec!["44".repeat(48)]);
        assert_eq!(resolved.binding_b64, Some(binding_b64));
    }

    #[test]
    fn test_resolve_effective_attestation_config_rejects_policy_outside_allowed_dirs() {
        let mut cfg = KmsAttestationConfig::default();
        cfg.enabled = true;
        cfg.allowed_tcb_statuses = vec!["Mock".to_owned()];
        cfg.allowed_mrtd = vec![format!("{:0>96}", "")];
        cfg.policy_json_path = Some(Utf8PathBuf::from("/etc/hosts"));

        let result = resolve_effective_attestation_config(&cfg);
        assert!(result.is_err());
        let error_text = result.unwrap_err().to_string();
        assert!(error_text.contains("must be under one of"));
    }

    fn make_runtime_attestation_config(accept_mock: bool) -> KmsAttestationConfig {
        let mut cfg = KmsAttestationConfig::default();
        cfg.enabled = true;
        cfg.accept_mock = accept_mock;
        cfg.allowed_tcb_statuses = vec!["Mock".to_owned()];
        cfg.allowed_mrtd = vec!["00".repeat(48)];
        cfg.allowed_rtmr0 = vec!["00".repeat(48)];
        cfg.allowed_rtmr1 = vec!["00".repeat(48)];
        cfg.allowed_rtmr2 = vec!["00".repeat(48)];
        cfg.allowed_rtmr3 = vec!["00".repeat(48)];
        cfg
    }

    #[tokio::test]
    async fn test_verify_kms_attestation_accepts_external_policy_json() {
        enable_mock_kms_attestation_env();

        let policy_file = write_temp_policy_file(&format!(
            r#"{{
  "allowed_tcb_statuses": ["Mock"],
  "allowed_mrtd": ["{measurement}"],
  "allowed_rtmr0": ["{measurement}"],
  "allowed_rtmr1": ["{measurement}"],
  "allowed_rtmr2": ["{measurement}"],
  "allowed_rtmr3": ["{measurement}"]
}}"#,
            measurement = "00".repeat(48)
        ));
        let policy_path = Utf8PathBuf::from_path_buf(policy_file.path().to_path_buf())
            .expect("temp policy path should be valid utf-8");

        let mut cfg = make_runtime_attestation_config(true);
        cfg.policy_json_path = Some(policy_path.clone());

        let client = reqwest::Client::new();
        let base_url = spawn_attest_server(AttestResponseMode::Valid).await;
        let result = verify_kms_attestation(&client, &base_url, &cfg).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_verify_kms_attestation_rejects_report_data_mismatch() {
        enable_mock_kms_attestation_env();

        let cfg = make_runtime_attestation_config(true);

        let client = reqwest::Client::new();
        let base_url = spawn_attest_server(AttestResponseMode::ReportDataMismatch).await;
        let result = verify_kms_attestation(&client, &base_url, &cfg).await;

        assert!(result.is_err());
        let error_text = result.unwrap_err().to_string();
        assert!(error_text.contains("reportData mismatch"));
    }

    #[tokio::test]
    async fn test_verify_kms_attestation_rejects_disallowed_measurement() {
        enable_mock_kms_attestation_env();

        let mut cfg = make_runtime_attestation_config(true);
        cfg.allowed_mrtd = vec!["ab".repeat(48)];

        let client = reqwest::Client::new();
        let base_url = spawn_attest_server(AttestResponseMode::Valid).await;
        let result = verify_kms_attestation(&client, &base_url, &cfg).await;

        assert!(result.is_err());
        let error_text = result.unwrap_err().to_string();
        assert!(error_text.contains("MRTD"));
    }

    #[tokio::test]
    async fn test_verify_kms_attestation_rejects_mock_quote_when_accept_mock_disabled() {
        enable_mock_kms_attestation_env();

        let cfg = make_runtime_attestation_config(false);

        let client = reqwest::Client::new();
        let base_url = spawn_attest_server(AttestResponseMode::Valid).await;
        let result = verify_kms_attestation(&client, &base_url, &cfg).await;

        assert!(result.is_err());
        let error_text = result.unwrap_err().to_string();
        assert!(error_text.contains("accept_mock is disabled"));
    }

    #[tokio::test]
    async fn test_probe_phala_flow_succeeds() {
        enable_mock_kms_attestation_env();
        let (base_url, get_key_hits) = spawn_probe_server(ProbeServerMode {
            attest: AttestResponseMode::Valid,
            challenge: ProbeChallengeMode::Valid,
            get_key: ProbeGetKeyMode::Success,
        })
        .await;
        let phala_config = make_probe_phala_config(&base_url, true);
        let identity = Keypair::generate_ed25519();
        let peer_id = identity.public().to_peer_id().to_base58();

        let key = probe_phala_storage_key_with_attestor(
            &phala_config,
            &peer_id,
            &identity,
            mock_probe_attestor,
        )
        .await
        .expect("probe flow should succeed");

        assert_eq!(key, vec![0x11u8; 32]);
        assert_eq!(get_key_hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_probe_phala_flow_rejects_spoofed_attest_before_key_fetch() {
        enable_mock_kms_attestation_env();
        let (base_url, get_key_hits) = spawn_probe_server(ProbeServerMode {
            attest: AttestResponseMode::ReportDataMismatch,
            challenge: ProbeChallengeMode::Valid,
            get_key: ProbeGetKeyMode::Success,
        })
        .await;
        let phala_config = make_probe_phala_config(&base_url, true);
        let identity = Keypair::generate_ed25519();
        let peer_id = identity.public().to_peer_id().to_base58();

        let err = probe_phala_storage_key_with_attestor(
            &phala_config,
            &peer_id,
            &identity,
            mock_probe_attestor,
        )
        .await
        .expect_err("reportData mismatch must fail probe");

        assert_eq!(err.stage, KmsProbeStage::Attest);
        assert_eq!(err.code, "KMS_ATTEST_REPORT_DATA_MISMATCH");
        assert_eq!(get_key_hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_probe_phala_flow_propagates_measurement_policy_rejection_with_stable_code() {
        enable_mock_kms_attestation_env();
        let (base_url, _get_key_hits) = spawn_probe_server(ProbeServerMode {
            attest: AttestResponseMode::Valid,
            challenge: ProbeChallengeMode::Valid,
            get_key: ProbeGetKeyMode::MeasurementPolicyRejected,
        })
        .await;
        let phala_config = make_probe_phala_config(&base_url, true);
        let identity = Keypair::generate_ed25519();
        let peer_id = identity.public().to_peer_id().to_base58();

        let err = probe_phala_storage_key_with_attestor(
            &phala_config,
            &peer_id,
            &identity,
            mock_probe_attestor,
        )
        .await
        .expect_err("measurement policy rejection must fail probe");

        assert_eq!(err.stage, KmsProbeStage::GetKey);
        assert_eq!(err.code, "KMS_PROFILE_POLICY_REJECTED");
        assert_eq!(
            err.kms_error.as_deref(),
            Some("measurement_policy_rejected")
        );
    }

    #[tokio::test]
    async fn test_probe_phala_flow_rejects_malformed_challenge_nonce() {
        enable_mock_kms_attestation_env();
        let (base_url, _get_key_hits) = spawn_probe_server(ProbeServerMode {
            attest: AttestResponseMode::Valid,
            challenge: ProbeChallengeMode::MalformedNonce,
            get_key: ProbeGetKeyMode::Success,
        })
        .await;
        let phala_config = make_probe_phala_config(&base_url, false);
        let identity = Keypair::generate_ed25519();
        let peer_id = identity.public().to_peer_id().to_base58();

        let err = probe_phala_storage_key_with_attestor(
            &phala_config,
            &peer_id,
            &identity,
            mock_probe_attestor,
        )
        .await
        .expect_err("malformed challenge nonce must fail probe");

        assert_eq!(err.stage, KmsProbeStage::Challenge);
        assert_eq!(err.code, "KMS_CHALLENGE_NONCE_INVALID");
    }

    #[tokio::test]
    async fn test_probe_phala_flow_rejects_oversized_challenge_nonce() {
        enable_mock_kms_attestation_env();
        let (base_url, _get_key_hits) = spawn_probe_server(ProbeServerMode {
            attest: AttestResponseMode::Valid,
            challenge: ProbeChallengeMode::OversizedNonce,
            get_key: ProbeGetKeyMode::Success,
        })
        .await;
        let phala_config = make_probe_phala_config(&base_url, false);
        let identity = Keypair::generate_ed25519();
        let peer_id = identity.public().to_peer_id().to_base58();

        let err = probe_phala_storage_key_with_attestor(
            &phala_config,
            &peer_id,
            &identity,
            mock_probe_attestor,
        )
        .await
        .expect_err("oversized challenge nonce must fail probe");

        assert_eq!(err.stage, KmsProbeStage::Challenge);
        assert_eq!(err.code, "KMS_CHALLENGE_NONCE_OVERSIZED");
    }

    #[tokio::test]
    async fn test_probe_storage_key_rejects_insecure_transport_in_strict_mode() {
        let phala_config = parse_phala_config(json!({
            "url": "http://kms.example.com/",
            "attestation": {
                "enabled": true,
                "accept_mock": false,
                "allowed_tcb_statuses": ["UpToDate"],
                "allowed_mrtd": ["00".repeat(48)],
                "allowed_rtmr0": ["00".repeat(48)],
                "allowed_rtmr1": ["00".repeat(48)],
                "allowed_rtmr2": ["00".repeat(48)],
                "allowed_rtmr3": ["00".repeat(48)]
            }
        }));
        let kms_config: KmsConfig = serde_json::from_value(json!({
            "phala": phala_config
        }))
        .expect("valid kms config");
        let identity = Keypair::generate_ed25519();
        let peer_id = identity.public().to_peer_id().to_base58();

        let result = probe_storage_key(&kms_config, &peer_id, &identity).await;
        assert!(!result.ok);
        assert_eq!(result.stage, KmsProbeStage::Transport);
        assert_eq!(result.code, "KMS_HTTP_INSECURE");
    }

    fn make_release_policy(
        verification_result: &VerificationResult,
        allowed_tcb_statuses: Vec<String>,
        allowed_mrtd: Vec<String>,
    ) -> KmsAttestationPolicy {
        let body = &verification_result.quote.body;
        KmsAttestationPolicy {
            allowed_tcb_statuses,
            allowed_mrtd,
            allowed_rtmr0: vec![normalize_attestation_measurement(&body.rtmr0)],
            allowed_rtmr1: vec![normalize_attestation_measurement(&body.rtmr1)],
            allowed_rtmr2: vec![normalize_attestation_measurement(&body.rtmr2)],
            allowed_rtmr3: vec![normalize_attestation_measurement(&body.rtmr3)],
            default_binding_b64: base64::engine::general_purpose::STANDARD.encode([0x22u8; 32]),
        }
    }

    fn make_mock_verification_result() -> VerificationResult {
        let nonce = [0x11u8; 32];
        let binding = [0x22u8; 32];
        let report_data = build_kms_attestation_report_data(&nonce, &binding);
        let quote_bytes = mock_quote_bytes_with_report_data(&report_data);
        verify_mock_attestation(&quote_bytes, &nonce, Some(&binding))
            .expect("mock verification result should be created")
    }

    #[test]
    fn test_enforce_attestation_policy_rejects_disallowed_tcb_status() {
        let verification_result = make_mock_verification_result();
        let policy = make_release_policy(
            &verification_result,
            vec!["uptodate".to_owned()],
            vec![normalize_attestation_measurement(
                &verification_result.quote.body.mrtd,
            )],
        );

        let err = enforce_attestation_policy(&policy, &verification_result, false)
            .expect_err("disallowed TCB status must fail")
            .to_string();
        assert!(err.contains("TCB status"));
    }

    #[test]
    fn test_enforce_attestation_policy_handles_missing_tcb_status_for_mock_quotes() {
        let mut verification_result = make_mock_verification_result();
        verification_result.tcb_status = None;
        let policy = make_release_policy(
            &verification_result,
            vec!["mock".to_owned()],
            vec![normalize_attestation_measurement(
                &verification_result.quote.body.mrtd,
            )],
        );

        assert!(enforce_attestation_policy(&policy, &verification_result, true).is_ok());
        assert!(enforce_attestation_policy(&policy, &verification_result, false).is_err());
    }

    #[test]
    fn test_enforce_attestation_policy_rejects_measurement_mismatch() {
        let verification_result = make_mock_verification_result();
        let policy = make_release_policy(
            &verification_result,
            vec!["mock".to_owned()],
            vec!["ab".repeat(48)],
        );

        let err = enforce_attestation_policy(&policy, &verification_result, true)
            .expect_err("mismatched MRTD must fail")
            .to_string();
        assert!(err.contains("MRTD"));
    }

    fn make_strict_runtime_policy(
        verification_result: &VerificationResult,
    ) -> NormalizedKmsAttestationPolicy {
        let body = &verification_result.quote.body;
        NormalizedKmsAttestationPolicy {
            accept_mock: false,
            allowed_tcb_statuses: vec!["mock".to_owned()],
            allowed_mrtd: vec![normalize_attestation_measurement(&body.mrtd)],
            allowed_rtmr0: vec![normalize_attestation_measurement(&body.rtmr0)],
            allowed_rtmr1: vec![normalize_attestation_measurement(&body.rtmr1)],
            allowed_rtmr2: vec![normalize_attestation_measurement(&body.rtmr2)],
            allowed_rtmr3: vec![normalize_attestation_measurement(&body.rtmr3)],
            binding: [0x22; 32],
            binding_b64: None,
        }
    }

    #[test]
    fn test_enforce_kms_attestation_policy_rejects_disallowed_tcb_status() {
        let verification_result = make_mock_verification_result();
        let mut policy = make_strict_runtime_policy(&verification_result);
        policy.allowed_tcb_statuses = vec!["uptodate".to_owned()];

        let err = enforce_kms_attestation_policy(&policy, &verification_result)
            .expect_err("disallowed TCB status must fail")
            .to_string();
        assert!(err.contains("TCB status"));
    }

    #[test]
    fn test_enforce_kms_attestation_policy_rejects_rtmr_mismatch_for_each_lane() {
        for lane in ["RTMR0", "RTMR1", "RTMR2", "RTMR3"] {
            let mut verification_result = make_mock_verification_result();
            let policy = make_strict_runtime_policy(&verification_result);

            match lane {
                "RTMR0" => verification_result.quote.body.rtmr0 = "ab".repeat(48),
                "RTMR1" => verification_result.quote.body.rtmr1 = "ab".repeat(48),
                "RTMR2" => verification_result.quote.body.rtmr2 = "ab".repeat(48),
                "RTMR3" => verification_result.quote.body.rtmr3 = "ab".repeat(48),
                _ => unreachable!(),
            }

            let err = enforce_kms_attestation_policy(&policy, &verification_result)
                .expect_err("RTMR mismatch must fail")
                .to_string();
            assert!(err.contains(lane));
        }
    }

    #[test]
    fn test_enforce_kms_attestation_policy_rejects_empty_required_allowlists_in_strict_mode() {
        let verification_result = make_mock_verification_result();

        let mut policy = make_strict_runtime_policy(&verification_result);
        policy.allowed_mrtd.clear();
        let err = enforce_kms_attestation_policy(&policy, &verification_result)
            .expect_err("empty MRTD allowlist must fail")
            .to_string();
        assert!(err.contains("allowed_mrtd"));

        let mut policy = make_strict_runtime_policy(&verification_result);
        policy.allowed_rtmr0.clear();
        let err = enforce_kms_attestation_policy(&policy, &verification_result)
            .expect_err("empty RTMR0 allowlist must fail")
            .to_string();
        assert!(err.contains("allowed_rtmr0"));

        let mut policy = make_strict_runtime_policy(&verification_result);
        policy.allowed_rtmr1.clear();
        let err = enforce_kms_attestation_policy(&policy, &verification_result)
            .expect_err("empty RTMR1 allowlist must fail")
            .to_string();
        assert!(err.contains("allowed_rtmr1"));

        let mut policy = make_strict_runtime_policy(&verification_result);
        policy.allowed_rtmr2.clear();
        let err = enforce_kms_attestation_policy(&policy, &verification_result)
            .expect_err("empty RTMR2 allowlist must fail")
            .to_string();
        assert!(err.contains("allowed_rtmr2"));

        let mut policy = make_strict_runtime_policy(&verification_result);
        policy.allowed_rtmr3.clear();
        let err = enforce_kms_attestation_policy(&policy, &verification_result)
            .expect_err("empty RTMR3 allowlist must fail")
            .to_string();
        assert!(err.contains("allowed_rtmr3"));
    }

    #[test]
    fn test_enforce_kms_attestation_policy_rejects_effectively_empty_allowlists_in_strict_mode() {
        let verification_result = make_mock_verification_result();

        let mut policy = make_strict_runtime_policy(&verification_result);
        policy.allowed_tcb_statuses = vec!["   ".to_owned()];
        let err = enforce_kms_attestation_policy(&policy, &verification_result)
            .expect_err("whitespace-only TCB status allowlist must fail")
            .to_string();
        assert!(err.contains("allowed_tcb_statuses"));

        let mut policy = make_strict_runtime_policy(&verification_result);
        policy.allowed_mrtd = vec!["0x".to_owned(), " ".to_owned()];
        let err = enforce_kms_attestation_policy(&policy, &verification_result)
            .expect_err("normalized-empty MRTD allowlist must fail")
            .to_string();
        assert!(err.contains("allowed_mrtd"));
    }

    #[test]
    fn test_enforce_kms_attestation_policy_allows_empty_tcb_allowlist_in_mock_mode() {
        let verification_result = make_mock_verification_result();
        let mut policy = make_strict_runtime_policy(&verification_result);
        policy.accept_mock = true;
        policy.allowed_tcb_statuses.clear();

        assert!(enforce_kms_attestation_policy(&policy, &verification_result).is_ok());
    }

    #[test]
    fn test_enforce_attestation_policy_rejects_empty_required_allowlists() {
        let verification_result = make_mock_verification_result();
        let mut policy = make_release_policy(
            &verification_result,
            vec!["mock".to_owned()],
            vec![normalize_attestation_measurement(
                &verification_result.quote.body.mrtd,
            )],
        );
        policy.allowed_rtmr0.clear();

        let err = enforce_attestation_policy(&policy, &verification_result, true)
            .expect_err("empty RTMR0 allowlist must fail")
            .to_string();
        assert!(err.contains("policy.allowed_rtmr0"));
    }
}
