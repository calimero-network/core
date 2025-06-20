extern crate lazy_static;

use thiserror::Error;

// pub mod api;
pub mod auth;
pub mod config;
pub mod providers;
// pub mod secrets;
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
    #[error("Storage error: {0}")]
    StorageError(String),
    #[error("Provider error: {0}")]
    ProviderError(String),
    #[error("Signature verification failed: {0}")]
    SignatureVerificationFailed(String),
    #[error("Key ownership verification failed: {0}")]
    KeyOwnershipFailed(String),
    #[error("Token generation failed: {0}")]
    TokenGenerationFailed(String),
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Rate limit exceeded: {0}")]
    RateLimitExceeded(String),
    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),
}
