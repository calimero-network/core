use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{HeaderMap, Request};
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

/// Authentication provider trait
///
/// This trait defines the interface for authentication providers.
/// Each provider implements a specific authentication method.
pub trait AuthProvider: Send + Sync {
    /// Get the name of the provider
    fn name(&self) -> &str;

    /// Verify a request and check permissions
    ///
    /// This method extracts data from the request, then performs async verification.
    /// The split approach avoids capturing references to Body in async code.
    fn verify_request(&self, request: &Request<Body>) -> eyre::Result<AuthRequestVerifier>;

    /// Verify a token request directly
    ///
    /// This method creates a verifier from a token request.
    ///
    /// # Arguments
    ///
    /// * `token_request` - The token request
    ///
    /// # Returns
    ///
    /// * `eyre::Result<AuthRequestVerifier>` - The verifier
    fn verify_token_request(
        &self,
        token_request: &api::handlers::auth::TokenRequest,
    ) -> eyre::Result<AuthRequestVerifier> {
        // Default implementation returns an error
        Err(eyre::eyre!(
            "Token request verification not supported by this provider"
        ))
    }
}

/// Auth request verifier
///
/// This holds the data needed to verify an authentication request without
/// holding any references to the original request.
pub struct AuthRequestVerifier {
    verifier: Box<dyn AuthVerifierFn>,
}

/// Auth verifier function trait
///
/// This trait defines an async function that performs authentication verification
/// without holding any references to the original request.
#[async_trait]
pub trait AuthVerifierFn: Send + Sync {
    /// Perform verification
    async fn verify(&self) -> Result<AuthResponse, AuthError>;
}

impl AuthRequestVerifier {
    /// Create a new verifier with the given function
    pub fn new<F>(verifier: F) -> Self
    where
        F: AuthVerifierFn + 'static,
    {
        Self {
            verifier: Box::new(verifier),
        }
    }

    /// Verify the request
    pub async fn verify(&self) -> Result<AuthResponse, AuthError> {
        self.verifier.verify().await
    }
}
