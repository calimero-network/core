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
    /// [`AuthError::InvalidToken`] so the HTTP layer can map it to `403
    /// Forbidden` by matching the variant rather than sniffing the message
    /// text — renaming a message can no longer silently downgrade a revoked
    /// token to a generic `401`.
    #[error("Token has been revoked")]
    TokenRevoked,
    #[error("Storage error: {message}")]
    StorageError {
        message: String,
        /// The underlying storage-layer error, preserved so the full cause
        /// chain survives (e.g. for `tracing`'s `error = %err` rendering and
        /// `std::error::Error::source` walking) instead of being flattened to
        /// a string.
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
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
    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),
}

impl AuthError {
    /// Wrap a concrete error as a [`AuthError::StorageError`], keeping it as the
    /// error `source` so the cause chain is not lost.
    pub fn storage<E>(source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        AuthError::StorageError {
            message: source.to_string(),
            source: Some(Box::new(source)),
        }
    }

    /// Like [`AuthError::storage`] but prefixes a human-readable context onto
    /// the message while still preserving the original error as the `source`.
    pub fn storage_context<E>(context: impl AsRef<str>, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        AuthError::StorageError {
            message: format!("{}: {source}", context.as_ref()),
            source: Some(Box::new(source)),
        }
    }
}
