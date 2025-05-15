use std::sync::Arc;

use axum::http::HeaderMap;

use crate::api::handlers::auth::TokenRequest;
use crate::providers::provider::AuthData;
use crate::{AuthError, AuthProvider, AuthResponse};

/// Authentication service
///
/// The service manages all the authentication providers and routes
/// authentication requests to the appropriate provider.
#[derive(Clone)]
pub struct AuthService {
    providers: Arc<Vec<Box<dyn AuthProvider>>>,
}

impl AuthService {
    /// Create a new authentication service
    ///
    /// # Arguments
    ///
    /// * `providers` - The authentication providers to use
    pub fn new(providers: Vec<Box<dyn AuthProvider>>) -> Self {
        Self {
            providers: Arc::new(providers),
        }
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
        // Find a provider that has a token manager
        for provider in self.providers.iter() {
            if let Some(near_provider) = provider.as_any().downcast_ref::<crate::providers::near_wallet::NearWalletProvider>() {
                // Use the token manager to verify the token
                return near_provider.get_token_manager().verify_token_from_headers(headers).await;
            }
        }
        
        // If no provider with a token manager is found
        Err(AuthError::InvalidRequest("No JWT token provider available".to_string()))
    }

    /// Authenticate a token request
    ///
    /// This method validates a token request against all providers.
    ///
    /// # Arguments
    ///
    /// * `token_request` - The token request
    ///
    /// # Returns
    ///
    /// * `Result<AuthResponse, AuthError>` - The result of the verification
    pub async fn authenticate_token_request(
        &self,
        token_request: &TokenRequest,
    ) -> Result<AuthResponse, AuthError> {
        // Convert the token request to AuthData and use the more direct approach
        let auth_data = match token_request.auth_method.as_str() {
            "near_wallet" => {
                // For NEAR wallet, create appropriate auth data
                let message = match &token_request.message {
                    Some(msg) => msg.as_bytes().to_vec(),
                    None => {
                        return Err(AuthError::InvalidRequest(
                            "Missing message for NEAR wallet authentication".to_string(),
                        ));
                    }
                };
                
                let account_id = match &token_request.wallet_address {
                    Some(addr) => addr.clone(),
                    None => {
                        return Err(AuthError::InvalidRequest(
                            "Missing wallet address for NEAR wallet authentication".to_string(),
                        ));
                    }
                };
                
                AuthData::NearWallet {
                    account_id,
                    public_key: token_request.public_key.clone(),
                    message,
                    signature: token_request.signature.clone(),
                }
            },
            // No other auth methods supported yet
            method => {
                return Err(AuthError::InvalidRequest(format!(
                    "Unsupported authentication method: {}",
                    method
                )));
            }
        };
        
        // Use the authenticate_with_data method which now uses the direct approach
        self.authenticate_with_data(auth_data).await
    }

    /// Authenticate using direct authentication data
    ///
    /// This method authenticates the user directly using the provided AuthData
    ///
    /// # Arguments
    ///
    /// * `auth_data` - The authentication data
    ///
    /// # Returns
    ///
    /// * `Result<AuthResponse, AuthError>` - The result of the authentication
    pub async fn authenticate_with_data(
        &self,
        auth_data: AuthData,
    ) -> Result<AuthResponse, AuthError> {
        // Select provider based on auth data type
        let provider = match &auth_data {
            AuthData::NearWallet { .. } => {
                self.providers.iter().find(|p| p.name() == "near_wallet")
            }
        };
        
        // Try to authenticate with the selected provider
        if let Some(provider) = provider {
            // Extract data from the AuthData enum
            match &auth_data {
                AuthData::NearWallet { account_id, public_key, message, signature } => {
                    // Get a reference to the specific provider (we already know it's a NearWalletProvider)
                    if let Some(near_provider) = provider.as_any().downcast_ref::<crate::providers::near_wallet::NearWalletProvider>() {
                        // Call the direct authentication method on the provider
                        return near_provider.authenticate_near_wallet(
                            account_id, 
                            public_key, 
                            message, 
                            signature
                        ).await;
                    }
                }
            }
        }

        Err(AuthError::AuthenticationFailed(
            "No valid authentication provider found for the provided data".to_string(),
        ))
    }

    /// Get the available providers
    ///
    /// # Returns
    ///
    /// * `&[Box<dyn AuthProvider>]` - The available providers
    pub fn providers(&self) -> &[Box<dyn AuthProvider>] {
        &self.providers
    }
}