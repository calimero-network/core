//! HTTP request handlers for the key release service.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use calimero_tee_attestation::{
    is_mock_quote, verify_attestation, verify_mock_attestation, VerificationResult,
};
use dstack_sdk::dstack_client::DstackClient;
use libp2p_identity::PublicKey;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, error, info, warn};

use crate::Config;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub challenges: Arc<Mutex<HashMap<String, PendingChallenge>>>,
}

#[derive(Debug, Clone)]
pub struct PendingChallenge {
    pub nonce: [u8; 32],
    pub peer_id: String,
    pub expires_at: u64,
}

/// Request body for the challenge endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChallengeRequest {
    /// Peer ID of the requesting merod node (base58 encoded).
    pub peer_id: String,
}

/// Response body for the challenge endpoint.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChallengeResponse {
    /// Unique challenge ID.
    pub challenge_id: String,
    /// Base64-encoded 32-byte nonce.
    pub nonce_b64: String,
    /// Expiration timestamp (unix seconds).
    pub expires_at: u64,
}

/// Request body for the get-key endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetKeyRequest {
    /// Challenge ID obtained from /challenge.
    pub challenge_id: String,
    /// Base64-encoded TDX attestation quote.
    pub quote_b64: String,
    /// Peer ID of the requesting merod node (base58 encoded).
    pub peer_id: String,
    /// Base64-encoded protobuf representation of libp2p public key.
    pub peer_public_key_b64: String,
    /// Base64-encoded signature over challenge payload.
    pub signature_b64: String,
}

/// Response body for the get-key endpoint.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetKeyResponse {
    /// Hex-encoded storage encryption key (as received from dstack).
    pub key: String,
}

/// Error response body.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResponse {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

/// Service-level errors.
#[derive(Debug)]
pub enum ServiceError {
    InvalidBase64(String),
    InvalidChallenge(String),
    InvalidPeerPublicKey(String),
    InvalidSignature(String),
    AttestationVerificationFailed(String),
    MockAttestationRejected,
    PeerIdentityMismatch,
    PeerIdMismatch,
    TcbStatusRejected(String),
    MeasurementPolicyRejected(String),
    KeyDerivationFailed(String),
}

impl IntoResponse for ServiceError {
    fn into_response(self) -> axum::response::Response {
        let (status, error_response) = match &self {
            ServiceError::InvalidBase64(msg) => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    error: "invalid_request".to_string(),
                    details: Some(msg.clone()),
                },
            ),
            ServiceError::InvalidChallenge(msg) => (
                StatusCode::UNAUTHORIZED,
                ErrorResponse {
                    error: "invalid_challenge".to_string(),
                    details: Some(msg.clone()),
                },
            ),
            ServiceError::InvalidPeerPublicKey(msg) => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    error: "invalid_peer_public_key".to_string(),
                    details: Some(msg.clone()),
                },
            ),
            ServiceError::InvalidSignature(msg) => (
                StatusCode::UNAUTHORIZED,
                ErrorResponse {
                    error: "invalid_signature".to_string(),
                    details: Some(msg.clone()),
                },
            ),
            ServiceError::AttestationVerificationFailed(msg) => (
                StatusCode::UNAUTHORIZED,
                ErrorResponse {
                    error: "attestation_verification_failed".to_string(),
                    details: Some(msg.clone()),
                },
            ),
            ServiceError::MockAttestationRejected => (
                StatusCode::UNAUTHORIZED,
                ErrorResponse {
                    error: "mock_attestation_rejected".to_string(),
                    details: Some(
                        "Mock attestations are not accepted in production mode".to_string(),
                    ),
                },
            ),
            ServiceError::PeerIdentityMismatch => (
                StatusCode::UNAUTHORIZED,
                ErrorResponse {
                    error: "peer_identity_mismatch".to_string(),
                    details: Some(
                        "The provided peer public key does not correspond to the claimed peer ID"
                            .to_string(),
                    ),
                },
            ),
            ServiceError::PeerIdMismatch => (
                StatusCode::UNAUTHORIZED,
                ErrorResponse {
                    error: "peer_id_mismatch".to_string(),
                    details: Some(
                        "The peer ID in the attestation does not match the claimed peer ID"
                            .to_string(),
                    ),
                },
            ),
            ServiceError::TcbStatusRejected(msg) => (
                StatusCode::FORBIDDEN,
                ErrorResponse {
                    error: "tcb_status_rejected".to_string(),
                    details: Some(msg.clone()),
                },
            ),
            ServiceError::MeasurementPolicyRejected(msg) => (
                StatusCode::FORBIDDEN,
                ErrorResponse {
                    error: "measurement_policy_rejected".to_string(),
                    details: Some(msg.clone()),
                },
            ),
            ServiceError::KeyDerivationFailed(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    error: "key_derivation_failed".to_string(),
                    details: Some(msg.clone()),
                },
            ),
        };

        (status, Json(error_response)).into_response()
    }
}

/// Create the router with all endpoints.
pub fn create_router(config: Config) -> Router {
    let state = AppState {
        config,
        challenges: Arc::new(Mutex::new(HashMap::new())),
    };

    Router::new()
        .route("/health", get(health_handler))
        .route("/challenge", post(challenge_handler))
        .route("/get-key", post(get_key_handler))
        .with_state(state)
}

/// Health check endpoint.
async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "alive",
        "service": "mero-kms-phala"
    }))
}

/// Handler for challenge issuance.
async fn challenge_handler(
    State(state): State<AppState>,
    Json(request): Json<ChallengeRequest>,
) -> Result<Json<ChallengeResponse>, ServiceError> {
    let mut nonce = [0u8; 32];
    OsRng.fill_bytes(&mut nonce);

    let challenge_id = create_challenge_id();
    let now = unix_now_secs()?;
    let expires_at = now.saturating_add(state.config.challenge_ttl_secs);

    let mut guard = state
        .challenges
        .lock()
        .map_err(|_| ServiceError::InvalidChallenge("challenge store lock poisoned".to_owned()))?;

    prune_expired_challenges(&mut guard, now);
    guard.insert(
        challenge_id.clone(),
        PendingChallenge {
            nonce,
            peer_id: request.peer_id,
            expires_at,
        },
    );

    Ok(Json(ChallengeResponse {
        challenge_id,
        nonce_b64: base64::engine::general_purpose::STANDARD.encode(nonce),
        expires_at,
    }))
}

/// Handler for the get-key endpoint.
///
/// Validates the TDX attestation and returns a deterministic storage encryption key.
async fn get_key_handler(
    State(state): State<AppState>,
    Json(request): Json<GetKeyRequest>,
) -> Result<Json<GetKeyResponse>, ServiceError> {
    info!(peer_id = %request.peer_id, "Received key release request");

    // Decode the base64 quote
    let quote_bytes = base64::engine::general_purpose::STANDARD
        .decode(&request.quote_b64)
        .map_err(|e| ServiceError::InvalidBase64(e.to_string()))?;

    debug!(quote_len = quote_bytes.len(), "Decoded quote");

    let challenge_nonce = consume_challenge(&state, &request.challenge_id, &request.peer_id)
        .map_err(|msg| {
            ServiceError::InvalidChallenge(format!("Challenge validation failed: {}", msg))
        })?;

    // Verify that request is signed by the claimed peer identity.
    verify_peer_signature(
        &request.peer_id,
        &request.peer_public_key_b64,
        &request.signature_b64,
        &request.challenge_id,
        &challenge_nonce,
        &quote_bytes,
    )?;

    // Check if this is a mock quote
    let is_mock = is_mock_quote(&quote_bytes);
    if is_mock {
        if state.config.accept_mock_attestation {
            warn!(
                peer_id = %request.peer_id,
                "Accepting mock attestation (development mode)"
            );
        } else {
            error!(
                peer_id = %request.peer_id,
                "Mock attestation rejected (production mode)"
            );
            return Err(ServiceError::MockAttestationRejected);
        }
    }

    // Create nonce from peer_id hash (SHA256 of peer_id string)
    // This is what merod should put in report_data[32..64] when generating attestation.
    let peer_id_hash = hash_peer_id(&request.peer_id);
    debug!(
        peer_id = %request.peer_id,
        peer_id_hash = %hex::encode(&peer_id_hash),
        "Created peer ID hash for verification"
    );

    // Verify the attestation
    let verification_result = if is_mock {
        verify_mock_attestation(&quote_bytes, &challenge_nonce, Some(&peer_id_hash))
            .map_err(|e| ServiceError::AttestationVerificationFailed(e.to_string()))?
    } else {
        verify_attestation(&quote_bytes, &challenge_nonce, Some(&peer_id_hash))
            .await
            .map_err(|e| ServiceError::AttestationVerificationFailed(e.to_string()))?
    };

    // Check if verification passed
    if !verification_result.is_valid() {
        error!(
            peer_id = %request.peer_id,
            quote_verified = verification_result.quote_verified,
            nonce_verified = verification_result.nonce_verified,
            app_hash_verified = ?verification_result.application_hash_verified,
            "Attestation verification failed"
        );

        if !verification_result.nonce_verified {
            // The challenge nonce doesn't match what's in the attestation.
            return Err(ServiceError::InvalidChallenge(
                "Attested nonce does not match issued challenge".to_owned(),
            ));
        }

        if verification_result.application_hash_verified == Some(false) {
            // The peer_id hash in report_data[32..64] doesn't match.
            return Err(ServiceError::PeerIdMismatch);
        }

        return Err(ServiceError::AttestationVerificationFailed(
            "Quote cryptographic verification failed".to_string(),
        ));
    }

    info!(
        peer_id = %request.peer_id,
        "Attestation verified successfully"
    );

    if !is_mock {
        enforce_attestation_policy(&state.config, &verification_result)?;
    } else {
        warn!("Skipping measurement policy checks for accepted mock attestation");
    }

    // Derive the key using dstack SDK (returns hex-encoded key)
    let key_path = format!("merod/storage/{}", request.peer_id);
    let client = DstackClient::new(Some(&state.config.dstack_socket_path));
    let key_response = client
        .get_key(Some(key_path), None)
        .await
        .map_err(|e| ServiceError::KeyDerivationFailed(e.to_string()))?;

    info!(
        peer_id = %request.peer_id,
        "Key derived successfully"
    );

    // Return the hex-encoded key directly (caller handles decoding)
    Ok(Json(GetKeyResponse {
        key: key_response.key,
    }))
}

/// Hash a peer ID string to create a 32-byte identity binding value.
///
/// The merod node should use the same hashing when creating the attestation,
/// putting this hash in report_data[32..64].
fn hash_peer_id(peer_id: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(peer_id.as_bytes());
    hasher.finalize().into()
}

fn unix_now_secs() -> Result<u64, ServiceError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|e| ServiceError::InvalidChallenge(format!("system clock error: {}", e)))
}

fn create_challenge_id() -> String {
    let mut raw = [0u8; 16];
    OsRng.fill_bytes(&mut raw);
    hex::encode(raw)
}

fn prune_expired_challenges(store: &mut HashMap<String, PendingChallenge>, now: u64) {
    store.retain(|_, challenge| challenge.expires_at > now);
}

fn consume_challenge(
    state: &AppState,
    challenge_id: &str,
    peer_id: &str,
) -> Result<[u8; 32], String> {
    let now = unix_now_secs().map_err(|e| format!("{:?}", e))?;
    let mut guard = state
        .challenges
        .lock()
        .map_err(|_| "challenge store lock poisoned".to_owned())?;
    prune_expired_challenges(&mut guard, now);

    let challenge = guard
        .remove(challenge_id)
        .ok_or_else(|| "challenge not found or expired".to_owned())?;
    if challenge.peer_id != peer_id {
        return Err("challenge peer mismatch".to_owned());
    }

    if challenge.expires_at <= now {
        return Err("challenge has expired".to_owned());
    }

    Ok(challenge.nonce)
}

fn verify_peer_signature(
    peer_id: &str,
    peer_public_key_b64: &str,
    signature_b64: &str,
    challenge_id: &str,
    challenge_nonce: &[u8; 32],
    quote_bytes: &[u8],
) -> Result<(), ServiceError> {
    let public_key_bytes = base64::engine::general_purpose::STANDARD
        .decode(peer_public_key_b64)
        .map_err(|e| ServiceError::InvalidPeerPublicKey(e.to_string()))?;
    let signature_bytes = base64::engine::general_purpose::STANDARD
        .decode(signature_b64)
        .map_err(|e| ServiceError::InvalidSignature(e.to_string()))?;

    let public_key = PublicKey::try_decode_protobuf(&public_key_bytes)
        .map_err(|e| ServiceError::InvalidPeerPublicKey(e.to_string()))?;
    let derived_peer_id = public_key.to_peer_id().to_base58();
    if derived_peer_id != peer_id {
        return Err(ServiceError::PeerIdentityMismatch);
    }

    let payload = build_signature_payload(challenge_id, challenge_nonce, quote_bytes, peer_id)?;
    if !public_key.verify(&payload, &signature_bytes) {
        return Err(ServiceError::InvalidSignature(
            "signature verification failed".to_owned(),
        ));
    }

    Ok(())
}

fn build_signature_payload(
    challenge_id: &str,
    challenge_nonce: &[u8; 32],
    quote_bytes: &[u8],
    peer_id: &str,
) -> Result<Vec<u8>, ServiceError> {
    let quote_hash = Sha256::digest(quote_bytes);
    serde_json::to_vec(&serde_json::json!({
        "challengeId": challenge_id,
        "challengeNonceHex": hex::encode(challenge_nonce),
        "quoteHashHex": hex::encode(quote_hash),
        "peerId": peer_id,
    }))
    .map_err(|e| ServiceError::InvalidSignature(format!("failed to serialize payload: {}", e)))
}

fn enforce_attestation_policy(
    config: &Config,
    verification_result: &VerificationResult,
) -> Result<(), ServiceError> {
    let policy = &config.attestation_policy;

    if !policy.enforce_measurement_policy {
        return Ok(());
    }

    let actual_tcb_status = verification_result.tcb_status.clone().ok_or_else(|| {
        ServiceError::TcbStatusRejected(
            "Quote verification did not provide a TCB status".to_owned(),
        )
    })?;
    let normalized_tcb_status = actual_tcb_status.to_ascii_lowercase();

    if !policy
        .allowed_tcb_statuses
        .iter()
        .any(|allowed| allowed == &normalized_tcb_status)
    {
        return Err(ServiceError::TcbStatusRejected(format!(
            "TCB status '{}' is not allowed. Allowed values: {}",
            actual_tcb_status,
            policy.allowed_tcb_statuses.join(", ")
        )));
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
) -> Result<(), ServiceError> {
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

    Err(ServiceError::MeasurementPolicyRejected(format!(
        "{} '{}' is not in allowlist",
        label, normalized_actual
    )))
}

fn normalize_measurement(value: &str) -> String {
    value.trim().trim_start_matches("0x").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AttestationPolicy;

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
    fn test_error_response_serialization() {
        let error = ErrorResponse {
            error: "test_error".to_string(),
            details: Some("Test details".to_string()),
        };
        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("test_error"));
        assert!(json.contains("Test details"));

        let error_no_details = ErrorResponse {
            error: "test_error".to_string(),
            details: None,
        };
        let json = serde_json::to_string(&error_no_details).unwrap();
        assert!(!json.contains("details"));
    }

    #[test]
    fn test_policy_rejects_tcb_status() {
        let nonce = [0x11; 32];
        let mut mock_quote = b"MOCK_TDX_QUOTE_V1".to_vec();
        let mut report_data = [0u8; 64];
        report_data[..32].copy_from_slice(&nonce);
        mock_quote.extend_from_slice(&report_data);
        mock_quote.resize(256, 0);

        let mut verification = verify_mock_attestation(&mock_quote, &nonce, None).unwrap();
        verification.tcb_status = Some("OutOfDate".to_owned());

        let config = Config {
            attestation_policy: AttestationPolicy {
                enforce_measurement_policy: true,
                allowed_tcb_statuses: vec!["uptodate".to_owned()],
                ..AttestationPolicy::default()
            },
            ..Config::default()
        };

        let result = enforce_attestation_policy(&config, &verification);
        assert!(matches!(result, Err(ServiceError::TcbStatusRejected(_))));
    }

    #[test]
    fn test_policy_rejects_untrusted_mrtd() {
        let nonce = [0x22; 32];
        let mut mock_quote = b"MOCK_TDX_QUOTE_V1".to_vec();
        let mut report_data = [0u8; 64];
        report_data[..32].copy_from_slice(&nonce);
        mock_quote.extend_from_slice(&report_data);
        mock_quote.resize(256, 0);

        let mut verification = verify_mock_attestation(&mock_quote, &nonce, None).unwrap();
        verification.tcb_status = Some("UpToDate".to_owned());

        let config = Config {
            attestation_policy: AttestationPolicy {
                enforce_measurement_policy: true,
                allowed_tcb_statuses: vec!["uptodate".to_owned()],
                allowed_mrtd: vec!["1".repeat(96)],
                ..AttestationPolicy::default()
            },
            ..Config::default()
        };

        let result = enforce_attestation_policy(&config, &verification);
        assert!(matches!(
            result,
            Err(ServiceError::MeasurementPolicyRejected(_))
        ));
    }

    #[test]
    fn test_policy_accepts_allowlisted_measurements() {
        let nonce = [0x33; 32];
        let mut mock_quote = b"MOCK_TDX_QUOTE_V1".to_vec();
        let mut report_data = [0u8; 64];
        report_data[..32].copy_from_slice(&nonce);
        mock_quote.extend_from_slice(&report_data);
        mock_quote.resize(256, 0);

        let mut verification = verify_mock_attestation(&mock_quote, &nonce, None).unwrap();
        verification.tcb_status = Some("UpToDate".to_owned());
        let zero_48b = "0".repeat(96);

        let config = Config {
            attestation_policy: AttestationPolicy {
                enforce_measurement_policy: true,
                allowed_tcb_statuses: vec!["uptodate".to_owned()],
                allowed_mrtd: vec![zero_48b.clone()],
                allowed_rtmr0: vec![zero_48b.clone()],
                allowed_rtmr1: vec![zero_48b.clone()],
                allowed_rtmr2: vec![zero_48b.clone()],
                allowed_rtmr3: vec![zero_48b],
            },
            ..Config::default()
        };

        let result = enforce_attestation_policy(&config, &verification);
        assert!(result.is_ok());
    }

    #[test]
    fn test_signature_payload_is_deterministic() {
        let challenge_id = "abc123";
        let nonce = [0x5a; 32];
        let quote = b"quote-bytes";
        let peer_id = "12D3KooWAbcdefghijklmnopqrstuvwxyz";

        let payload1 = build_signature_payload(challenge_id, &nonce, quote, peer_id).unwrap();
        let payload2 = build_signature_payload(challenge_id, &nonce, quote, peer_id).unwrap();

        assert_eq!(payload1, payload2);
    }
}
