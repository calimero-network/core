use std::any::Any;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::Request;
use eyre::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::handlers::auth::TokenRequest;
use crate::{AuthError, AuthResponse};

/// Authentication provider trait
///
/// This trait defines the interface for authentication providers.
/// Each provider implements a specific authentication method.
pub trait AuthProvider: Send + Sync {
    /// Get the name of the provider
    fn name(&self) -> &str;

    /// Get the provider type (for categorization)
    fn provider_type(&self) -> &str;

    /// Get a description of this provider
    fn description(&self) -> &str;

    /// Check if this provider supports the given authentication method
    fn supports_method(&self, method: &str) -> bool;

    /// Check if the provider is properly configured and ready to use
    fn is_configured(&self) -> bool;

    /// Get provider-specific configuration options
    fn get_config_options(&self) -> serde_json::Value;

    /// Convert a TokenRequest to provider-specific auth data JSON
    ///
    /// This method allows providers to extract and format data according to their needs
    fn prepare_auth_data(&self, token_request: &TokenRequest) -> Result<Value, AuthError>;

    /// Create a verifier from parsed auth data
    ///
    /// This method creates a verifier that can authenticate the user based on the auth data.
    /// Each provider implements this differently based on their authentication requirements.
    ///
    /// # Arguments
    ///
    /// * `method` - The auth method to use
    /// * `auth_data` - The parsed auth data from the registry
    ///
    /// # Returns
    ///
    /// * `Result<AuthRequestVerifier, AuthError>` - A verifier that can authenticate the user
    fn create_verifier(
        &self,
        method: &str,
        auth_data: Box<dyn Any + Send + Sync>,
    ) -> Result<AuthRequestVerifier, AuthError>;

    /// Verify a request and check permissions
    ///
    /// This method extracts data from the request, then performs async verification.
    /// The split approach avoids capturing references to Body in async code.
    fn verify_request(&self, request: &Request<Body>) -> eyre::Result<AuthRequestVerifier>;

    /// Get provider-specific health status
    fn get_health_status(&self) -> eyre::Result<serde_json::Value> {
        // Default implementation returns basic health info
        Ok(serde_json::json!({
            "name": self.name(),
            "type": self.provider_type(),
            "configured": self.is_configured(),
        }))
    }

    /// Convert to Any for downcasting
    ///
    /// This is used to downcast to specific provider implementations.
    fn as_any(&self) -> &dyn Any;
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

/// Auth request verifier
///
/// This holds the data needed to verify an authentication request without
/// holding any references to the original request.
pub struct AuthRequestVerifier {
    verifier: Box<dyn AuthVerifierFn>,
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
