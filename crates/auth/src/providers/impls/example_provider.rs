use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::Request;
use eyre::Result;
use serde_json::Value;
use tracing::warn;

use crate::api::handlers::auth::TokenRequest;
use crate::auth::token::TokenManager;
use crate::config::AuthConfig;
use crate::providers::core::provider::{AuthProvider, AuthRequestVerifier, AuthVerifierFn};
use crate::providers::core::provider_registry::ProviderRegistration;
use crate::secrets::SecretManager;
use crate::storage::{KeyStorage, MemoryStorage};
use crate::{register_auth_provider, AuthError, AuthResponse, RequestValidator};

/// Example provider for demonstration purposes
pub struct ExampleProvider {
    storage: Arc<dyn KeyStorage>,
    token_manager: TokenManager,
}

impl ExampleProvider {
    /// Create a new example provider
    pub fn new(storage: Arc<dyn KeyStorage>, token_manager: TokenManager) -> Self {
        Self {
            storage,
            token_manager,
        }
    }
}

#[async_trait]
impl<B: Send + Sync> RequestValidator<B> for ExampleProvider {
    async fn validate_request(&self, _request: &Request<B>) -> Result<AuthResponse, AuthError> {
        // In a real provider, this would validate authentication credentials
        // For this example, we just return a dummy error
        Err(AuthError::AuthenticationFailed(
            "Example provider is not yet implemented".to_string(),
        ))
    }
}

impl Clone for ExampleProvider {
    fn clone(&self) -> Self {
        Self {
            storage: self.storage.clone(),
            token_manager: self.token_manager.clone(),
        }
    }
}

impl AuthProvider for ExampleProvider {
    fn name(&self) -> &str {
        "example"
    }

    fn provider_type(&self) -> &str {
        "example"
    }

    fn description(&self) -> &str {
        "Example authentication provider for demonstration"
    }

    fn supports_method(&self, method: &str) -> bool {
        method == "example"
    }

    fn is_configured(&self) -> bool {
        true
    }

    fn get_config_options(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "example",
            "configurable_options": []
        })
    }

    fn prepare_auth_data(&self, token_request: &TokenRequest) -> Result<Value, AuthError> {
        // Example provider has simpler validation requirements
        // Just check that we have a public key and signature
        if token_request.public_key.is_empty() {
            return Err(AuthError::InvalidRequest(
                "Public key is required for example authentication".to_string(),
            ));
        }

        if token_request.signature.is_empty() {
            return Err(AuthError::InvalidRequest(
                "Signature is required for example authentication".to_string(),
            ));
        }

        // Create a simple JSON structure with just what this provider needs
        Ok(serde_json::json!({
            "public_key": token_request.public_key,
            "signature": token_request.signature,
            "timestamp": token_request.timestamp,
            "client_name": token_request.client_name
        }))
    }

    fn create_verifier(
        &self,
        method: &str,
        _auth_data: Box<dyn Any + Send + Sync>,
    ) -> Result<AuthRequestVerifier, AuthError> {
        // Only handle supported methods
        if !self.supports_method(method) {
            return Err(AuthError::InvalidRequest(format!(
                "Provider {} does not support method {}",
                self.name(),
                method
            )));
        }

        // For the example provider, we don't need to process the auth data
        // We simply create a verifier that returns a dummy response
        Ok(AuthRequestVerifier::new(ExampleVerifier))
    }

    fn verify_request(&self, _request: &Request<Body>) -> eyre::Result<AuthRequestVerifier> {
        // In a real provider, this would parse the request and verify credentials
        // For this example, we return a dummy verifier
        warn!("Example provider received verification request but is not yet implemented");

        Ok(AuthRequestVerifier::new(ExampleVerifier))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Example verifier
struct ExampleVerifier;

#[async_trait]
impl AuthVerifierFn for ExampleVerifier {
    async fn verify(&self) -> Result<AuthResponse, AuthError> {
        // In a real provider, this would actually verify credentials
        Err(AuthError::AuthenticationFailed(
            "Example provider is not yet implemented".to_string(),
        ))
    }
}

/// Registration for the example provider
#[derive(Clone)]
pub struct ExampleProviderRegistration;

impl ProviderRegistration for ExampleProviderRegistration {
    fn provider_id(&self) -> &str {
        "example"
    }

    fn create_provider(
        &self,
        storage: Arc<dyn KeyStorage>,
        config: &AuthConfig,
        token_manager: TokenManager,
    ) -> Result<Box<dyn AuthProvider>, eyre::Error> {
        let provider = ExampleProvider::new(storage, token_manager);
        Ok(Box::new(provider))
    }

    fn is_enabled(&self, config: &AuthConfig) -> bool {
        // Check if this provider is enabled in the config
        config
            .providers
            .get("example")
            .copied() // Get the bool value directly
            .unwrap_or(false)
    }
}

// Self-register the provider
register_auth_provider!(ExampleProviderRegistration);
