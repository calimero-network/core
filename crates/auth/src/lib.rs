#[macro_use]
extern crate lazy_static;

use async_trait::async_trait;
use axum::http::Request;
use eyre::Result;
use thiserror::Error;

pub mod api;
pub mod auth;
pub mod config;
pub mod providers;
pub mod server;
pub mod storage;
pub mod utils;

pub use auth::{forward_auth_middleware, AuthService};
pub use providers::core::provider::{AuthProvider, AuthRequestVerifier, AuthVerifierFn};

/// Response from an authentication validation attempt
#[derive(Debug, Clone)]
pub struct AuthResponse {
    /// Whether the authentication is valid
    pub is_valid: bool,
    /// The identifier of the authenticated user
    pub key_id: Option<String>,
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

/// Request validation trait for generic types
#[async_trait]
pub trait RequestValidator<B> {
    /// Verify the authentication and check permissions for a specific body type
    ///
    /// # Arguments
    ///
    /// * `request` - The request to verify
    ///
    /// # Returns
    ///
    /// * `Result<AuthResponse, AuthError>` - The result of the verification
    async fn validate_request(&self, request: &Request<B>) -> Result<AuthResponse, AuthError>;
}
