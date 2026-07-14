extern crate lazy_static;

use thiserror::Error;

pub mod api;
pub mod auth;
pub mod config;
pub mod embedded;
pub mod providers;
pub mod secrets;
pub mod server;
pub mod storage;
pub mod utils;

pub use auth::AuthService;
pub use providers::core::provider::{AuthProvider, AuthRequestVerifier, AuthVerifierFn};

/// Response from an authentication validation attempt
#[derive(Debug, Clone)]
pub struct AuthResponse {
    /// Whether the authentication is valid
    pub is_valid: bool,
    /// The identifier of the authenticated user
    pub key_id: String,
    /// The permissions granted to the authenticated user
    pub permissions: Vec<String>,
}

/// Error that can occur during authentication
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),
    #[error("Authorization failed: {0}")]
    AuthorizationFailed(String),
    #[error("Invalid token: {0}")]
    InvalidToken(String),
    #[error("Token has expired")]
    TokenExpired,
    /// The presented token's key has been revoked. Kept distinct from
    /// [`InvalidToken`](AuthError::InvalidToken) so the HTTP layer maps it to
    /// `403 Forbidden` via the type, not a substring match on the message
    /// (renaming the message must never silently downgrade revoked → `401`).
    #[error("Token has been revoked")]
    TokenRevoked,
    #[error("Storage error: {0}")]
    StorageError(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("Provider error: {0}")]
    ProviderError(String),
    #[error("Signature verification failed: {0}")]
    SignatureVerificationFailed(String),
    #[error("Key ownership verification failed: {0}")]
    KeyOwnershipFailed(String),
    #[error("Token generation failed: {0}")]
    TokenGenerationFailed(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),
}
