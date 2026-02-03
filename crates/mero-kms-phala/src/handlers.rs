//! HTTP request handlers for the key release service.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use calimero_tee_attestation::{is_mock_quote, verify_attestation, verify_mock_attestation};
use dstack_sdk::dstack_client::DstackClient;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, error, info, warn};

use crate::Config;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub config: Config,
}

/// Request body for the get-key endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetKeyRequest {
    /// Base64-encoded TDX attestation quote.
    pub quote_b64: String,
    /// Peer ID of the requesting merod node (base58 encoded).
    pub peer_id: String,
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
    AttestationVerificationFailed(String),
    MockAttestationRejected,
    PeerIdMismatch,
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
    let state = AppState { config };

    Router::new()
        .route("/health", get(health_handler))
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
    // This is what merod should put in report_data[0..32] when generating attestation
    let peer_id_hash = hash_peer_id(&request.peer_id);
    debug!(
        peer_id = %request.peer_id,
        peer_id_hash = %hex::encode(&peer_id_hash),
        "Created peer ID hash for verification"
    );

    // Verify the attestation
    let verification_result = if is_mock {
        verify_mock_attestation(&quote_bytes, &peer_id_hash, None)
            .map_err(|e| ServiceError::AttestationVerificationFailed(e.to_string()))?
    } else {
        verify_attestation(&quote_bytes, &peer_id_hash, None)
            .await
            .map_err(|e| ServiceError::AttestationVerificationFailed(e.to_string()))?
    };

    // Check if verification passed
    if !verification_result.is_valid() {
        error!(
            peer_id = %request.peer_id,
            quote_verified = verification_result.quote_verified,
            nonce_verified = verification_result.nonce_verified,
            "Attestation verification failed"
        );

        if !verification_result.nonce_verified {
            // The peer_id hash doesn't match what's in the attestation
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

/// Hash a peer ID string to create a 32-byte nonce.
///
/// The merod node should use the same hashing when creating the attestation,
/// putting this hash in report_data[0..32].
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
}
