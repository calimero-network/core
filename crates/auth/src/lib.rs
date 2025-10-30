extern crate lazy_static;

use std::sync::Arc;
use thiserror::Error;

pub mod api;
pub mod auth;
pub mod config;
pub mod providers;
pub mod secrets;
pub mod server;
pub mod storage;
pub mod utils;

// Register Farcaster provider and auth data type
use crate::providers::core::provider_data_registry::register_auth_data_type;
use crate::providers::core::provider_registry::register_provider;
use crate::providers::impls::farcaster::FarcasterProviderRegistration;
use crate::providers::impls::farcaster_auth_data::FarcasterAuthDataType;

// Register providers and auth data types at module initialization
lazy_static::lazy_static! {
    static ref _FARCASTER_PROVIDER: () = {
        register_provider(Arc::new(FarcasterProviderRegistration::new()));
    };
    static ref _FARCASTER_AUTH_DATA: () = {
        register_auth_data_type(Box::new(FarcasterAuthDataType::new()));
    };
}

// Force registration by referencing the lazy statics
fn _register_farcaster_components() {
    let _ = *_FARCASTER_PROVIDER;
    let _ = *_FARCASTER_AUTH_DATA;
}

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
