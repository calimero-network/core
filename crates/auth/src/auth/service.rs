use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{HeaderMap, Request};

use crate::AuthError;
use crate::AuthResponse;
use crate::AuthProvider;
use crate::AuthRequestVerifier;
use crate::api::handlers::auth::TokenRequest;

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

    /// Verify the authentication and check permissions
    ///
    /// This method tries all providers in order until one succeeds.
    ///
    /// # Arguments
    ///
    /// * `request` - The request to verify
    ///
    /// # Returns
    ///
    /// * `Result<AuthResponse, AuthError>` - The result of the verification
    pub async fn verify_and_check_permissions<B>(
        &self,
        request: &Request<B>,
    ) -> Result<AuthResponse, AuthError>
    where
        B: Send + 'static,
    {
        // Create a request with an empty body but with the same headers
        let mut builder = Request::builder()
            .method(request.method().clone())
            .uri(request.uri().clone());

        // Copy all headers
        for (name, value) in request.headers() {
            builder = builder.header(name, value);
        }

        // Build the request with an empty body
        let empty_body_req = builder
            .body(Body::empty())
            .unwrap_or_else(|_| Request::new(Body::empty()));

        // Try each provider in order
        let mut last_error = None;

        for provider in self.providers.iter() {
            match provider.verify_request(&empty_body_req) {
                Ok(verifier) => match verifier.verify().await {
                    Ok(response) => return Ok(response),
                    Err(err) => last_error = Some(err),
                },
                Err(_) => continue,
            }
        }

        Err(last_error.unwrap_or(AuthError::AuthenticationFailed(
            "No valid authentication provider found".to_string(),
        )))
    }

    /// Verify tokens from headers directly
    ///
    /// This method extracts the token from headers and verifies it.
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
        // Create a request with an empty body but with the provided headers
        let mut builder = Request::builder();

        // Copy all headers
        for (name, value) in headers {
            builder = builder.header(name, value);
        }

        // Build the request with an empty body
        let empty_body_req = builder
            .body(Body::empty())
            .unwrap_or_else(|_| Request::new(Body::empty()));

        // Try each provider in order
        let mut last_error = None;

        for provider in self.providers.iter() {
            match provider.verify_request(&empty_body_req) {
                Ok(verifier) => match verifier.verify().await {
                    Ok(response) => return Ok(response),
                    Err(err) => last_error = Some(err),
                },
                Err(_) => continue,
            }
        }

        Err(last_error.unwrap_or(AuthError::AuthenticationFailed(
            "No valid authentication provider found".to_string(),
        )))
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
        // Try each provider in order
        let mut last_error = None;

        for provider in self.providers.iter() {
            if provider.name() == token_request.auth_method {
                // Create a validation context with the token request
                let verification_result = provider.verify_token_request(token_request);

                match verification_result {
                    Ok(verifier) => match verifier.verify().await {
                        Ok(response) => return Ok(response),
                        Err(err) => last_error = Some(err),
                    },
                    Err(err) => last_error = Some(AuthError::ProviderError(err.to_string())),
                }
            }
        }

        Err(last_error.unwrap_or(AuthError::AuthenticationFailed(
            "No valid authentication provider found".to_string(),
        )))
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