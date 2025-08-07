use std::sync::Arc;

use axum::http::HeaderMap;
use serde_json::Value;

use crate::api::handlers::auth::TokenRequest;
use crate::auth::token::TokenManager;
use crate::providers::core::provider_data_registry;
use crate::{AuthError, AuthProvider, AuthResponse};

/// Authentication service
///
/// The service manages all the authentication providers and routes
/// authentication requests to the appropriate provider.
#[derive(Clone)]
pub struct AuthService {
    providers: Arc<Vec<Box<dyn AuthProvider>>>,
    token_manager: TokenManager,
}

impl AuthService {
    /// Create a new authentication service
    ///
    /// # Arguments
    ///
    /// * `providers` - The authentication providers to use
    /// * `token_manager` - The JWT token manager
    pub fn new(providers: Vec<Box<dyn AuthProvider>>, token_manager: TokenManager) -> Self {
        Self {
            providers: Arc::new(providers),
            token_manager,
        }
    }

    /// Get the token manager
    ///
    /// # Returns
    ///
    /// * `&TokenManager` - Reference to the token manager
    pub fn get_token_manager(&self) -> &TokenManager {
        &self.token_manager
    }

    /// Verify tokens from headers directly
    ///
    /// This method extracts and validates JWT tokens from the Authorization header.
    ///
    /// # Arguments
    ///
    /// * `headers` - The request headers
    ///
    /// # Returns
    ///
    /// * `Result<AuthResponse, AuthError>` - The result of the verification
    pub async fn verify_token_from_headers(
        &self,
        headers: &HeaderMap,
    ) -> Result<AuthResponse, AuthError> {
        self.token_manager.verify_token_from_headers(headers).await
    }

    /// Authenticate a token request
    ///
    /// This method authenticates the user using the provided token request
    ///
    /// # Arguments
    ///
    /// * `token_request` - The token request
    /// * `node_url` - The node URL this authentication is for (optional)
    ///
    /// # Returns
    ///
    /// * `Result<AuthResponse, AuthError>` - The result of the verification
    pub async fn authenticate_token_request(
        &self,
        token_request: &TokenRequest,
        node_url: Option<&str>,
    ) -> Result<AuthResponse, AuthError> {
        let auth_method = &token_request.auth_method;

        // Find a provider that supports this auth method
        let provider = self
            .providers
            .iter()
            .find(|p| p.supports_method(auth_method))
            .ok_or_else(|| {
                AuthError::InvalidRequest(format!(
                    "Unsupported authentication method: {}",
                    auth_method
                ))
            })?;

        // The provider prepares the auth data based on the token request
        // Each provider implements its own logic for this, so we don't need special cases here
        let auth_data_json = provider
            .prepare_auth_data(token_request)
            .map_err(|e| AuthError::InvalidRequest(e.to_string()))?;

        // Use the auth data registry to parse auth data to the correct type
        let auth_response = self
            .authenticate_with_data(auth_method, auth_data_json)
            .await?;

        // If node_id is provided, validate that the key is valid for this node
        if let Some(node_url) = node_url {
            if let Some(key) = self
                .token_manager
                .get_key_manager()
                .get_key(&auth_response.key_id)
                .await
                .map_err(|e| AuthError::AuthenticationFailed(e.to_string()))?
            {
                if !key.is_valid_for_node(Some(node_url)) {
                    return Err(AuthError::AuthenticationFailed(
                        "Key is not valid for this node".to_string(),
                    ));
                }
            }
        }

        Ok(auth_response)
    }

    /// Authenticate using parsed auth data
    ///
    /// This method authenticates the user using the auth method's registered handler
    ///
    /// # Arguments
    ///
    /// * `auth_method` - The authentication method
    /// * `auth_data_json` - The auth data as JSON value
    ///
    /// # Returns
    ///
    /// * `Result<AuthResponse, AuthError>` - The result of the authentication
    pub async fn authenticate_with_data(
        &self,
        auth_method: &str,
        auth_data_json: Value,
    ) -> Result<AuthResponse, AuthError> {
        // Find a provider that supports this auth method
        let provider = self
            .providers
            .iter()
            .find(|p| p.supports_method(auth_method))
            .ok_or_else(|| {
                AuthError::InvalidRequest(format!(
                    "Unsupported authentication method: {}",
                    auth_method
                ))
            })?;

        // Parse the auth data using our registry
        let auth_data = provider_data_registry::parse_auth_data(auth_method, auth_data_json)
            .map_err(|e| AuthError::InvalidRequest(e.to_string()))?;

        // Create a verifier from the provider and let it handle the authentication
        let verifier = provider
            .create_verifier(auth_method, auth_data)
            .map_err(|e| AuthError::AuthenticationFailed(e.to_string()))?;

        // Execute the verification process
        verifier
            .verify()
            .await
            .map_err(|e| AuthError::AuthenticationFailed(e.to_string()))
    }

    /// Get the available providers
    ///
    /// # Returns
    ///
    /// * `&[Box<dyn AuthProvider>]` - The available providers
    pub fn providers(&self) -> &[Box<dyn AuthProvider>] {
        &self.providers
    }

    /// Get a specific provider by name
    ///
    /// # Arguments
    ///
    /// * `provider_name` - The name of the provider to get
    ///
    /// # Returns
    ///
    /// * `Option<&Box<dyn AuthProvider>>` - The provider if found
    pub fn get_provider(&self, provider_name: &str) -> Option<&Box<dyn AuthProvider>> {
        self.providers.iter().find(|p| p.name() == provider_name)
    }
}
