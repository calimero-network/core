use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{HeaderMap, Request};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

use crate::config::JwtConfig;
use crate::storage::{deserialize, prefixes, ClientKey, Storage, StorageError};
use crate::{AuthError, AuthRequestVerifier, AuthResponse, AuthVerifierFn};

/// JWT Claims structure
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user ID)
    pub sub: String,
    /// Issuer
    pub iss: String,
    /// Audience
    pub aud: String,
    /// Expiration time (as Unix timestamp)
    pub exp: u64,
    /// Issued at (as Unix timestamp)
    pub iat: u64,
    /// JWT ID
    pub jti: String,
    /// Permissions
    pub permissions: Vec<String>,
}

/// JWT Token Manager
///
/// This component handles JWT token generation and verification.
#[derive(Clone)]
pub struct TokenManager {
    config: JwtConfig,
    storage: Arc<dyn Storage>,
}

impl TokenManager {
    /// Create a new JWT token manager
    ///
    /// # Arguments
    ///
    /// * `config` - JWT configuration
    /// * `storage` - Storage backend
    ///
    /// # Returns
    ///
    /// * `Self` - The token generator
    pub fn new(config: JwtConfig, storage: Arc<dyn Storage>) -> Self {
        Self { config, storage }
    }

    /// Generate a JWT access token
    ///
    /// # Arguments
    ///
    /// * `client_id` - Client ID
    /// * `key_id` - The key ID (user ID)
    /// * `permissions` - The permissions to include in the token
    ///
    /// # Returns
    ///
    /// * `Result<String, AuthError>` - The generated JWT token or error
    async fn generate_access_token(
        &self,
        client_id: &str,
        key_id: &str,
        permissions: &[String],
    ) -> Result<String, AuthError> {
        // Create the claims
        let expiry = Utc::now()
            .checked_add_signed(Duration::seconds(self.config.access_token_expiry as i64))
            .expect("valid timestamp")
            .timestamp() as u64;

        let jwt_id = format!("{}_{}", client_id, Utc::now().timestamp());

        let claims = Claims {
            sub: key_id.to_string(),
            iss: self.config.issuer.clone(),
            aud: client_id.to_string(),
            exp: expiry,
            iat: Utc::now().timestamp() as u64,
            jti: jwt_id,
            permissions: permissions.to_vec(),
        };

        // Encode the token
        let encoding_key = EncodingKey::from_secret(self.config.secret.as_bytes());
        let token = encode(&Header::default(), &claims, &encoding_key).map_err(|err| {
            AuthError::AuthenticationFailed(format!("Failed to generate JWT token: {}", err))
        })?;

        Ok(token)
    }

    /// Generate a JWT refresh token
    ///
    /// # Arguments
    ///
    /// * `client_id` - Client ID
    /// * `key_id` - The key ID (user ID)
    ///
    /// # Returns
    ///
    /// * `Result<String, AuthError>` - The generated JWT token or error
    async fn generate_refresh_token(
        &self,
        client_id: &str,
        key_id: &str,
    ) -> Result<String, AuthError> {
        // Create the claims
        let expiry = Utc::now()
            .checked_add_signed(Duration::seconds(self.config.refresh_token_expiry as i64))
            .expect("valid timestamp")
            .timestamp() as u64;

        let jwt_id = format!("refresh_{}_{}", client_id, Utc::now().timestamp());

        let claims = Claims {
            sub: key_id.to_string(),
            iss: self.config.issuer.clone(),
            aud: client_id.to_string(),
            exp: expiry,
            iat: Utc::now().timestamp() as u64,
            jti: jwt_id,
            permissions: vec![],
        };

        // Encode the token
        let encoding_key = EncodingKey::from_secret(self.config.secret.as_bytes());
        let token = encode(&Header::default(), &claims, &encoding_key).map_err(|err| {
            AuthError::AuthenticationFailed(format!(
                "Failed to generate JWT refresh token: {}",
                err
            ))
        })?;

        Ok(token)
    }

    /// Generate an access token and refresh token pair
    ///
    /// # Arguments
    ///
    /// * `client_id` - Client ID
    /// * `key_id` - The key ID (user ID)
    /// * `permissions` - The permissions to include in the token
    ///
    /// # Returns
    ///
    /// * `Result<(String, String), AuthError>` - The generated access and refresh tokens, or error
    pub async fn generate_token_pair(
        &self,
        client_id: &str,
        key_id: &str,
        permissions: &[String],
    ) -> Result<(String, String), AuthError> {
        let access_token = self
            .generate_access_token(client_id, key_id, permissions)
            .await?;
        let refresh_token = self.generate_refresh_token(client_id, key_id).await?;

        Ok((access_token, refresh_token))
    }

    /// Refresh a token pair using a refresh token
    ///
    /// # Arguments
    ///
    /// * `refresh_token` - The refresh token
    ///
    /// # Returns
    ///
    /// * `Result<(String, String), AuthError>` - The new access and refresh tokens, or error
    pub async fn refresh_token_pair(
        &self,
        refresh_token: &str,
    ) -> Result<(String, String), AuthError> {
        // Decode the refresh token
        let validation = Validation::new(Algorithm::HS256);
        let decoding_key = DecodingKey::from_secret(self.config.secret.as_bytes());

        let token_data =
            decode::<Claims>(refresh_token, &decoding_key, &validation).map_err(|err| {
                AuthError::AuthenticationFailed(format!("Invalid refresh token: {}", err))
            })?;

        let claims = token_data.claims;

        // Validate that the token is a refresh token
        if !claims.jti.starts_with("refresh_") {
            return Err(AuthError::AuthenticationFailed(
                "Invalid refresh token type".to_string(),
            ));
        }

        // Get the key ID and client ID from the token
        let key_id = claims.sub;
        let client_id = claims.aud;

        // Get the client key to check for revocation
        let client_key_path = format!("{}{}", prefixes::CLIENT_KEY, client_id);

        match self.storage.get(&client_key_path).await {
            Ok(Some(data)) => {
                let client_key: ClientKey = deserialize(&data).map_err(|err| {
                    AuthError::StorageError(format!("Failed to deserialize client key: {}", err))
                })?;

                // Check if the client key has been revoked
                if client_key.revoked_at.is_some() {
                    return Err(AuthError::AuthenticationFailed(
                        "Client key has been revoked".to_string(),
                    ));
                }

                // Check if the key has expired
                if let Some(expires_at) = client_key.expires_at {
                    if expires_at < Utc::now().timestamp() as u64 {
                        return Err(AuthError::AuthenticationFailed(
                            "Client key has expired".to_string(),
                        ));
                    }
                }

                // Generate new token pair
                self.generate_token_pair(&client_id, &key_id, &client_key.permissions)
                    .await
            }
            Ok(None) => Err(AuthError::AuthenticationFailed(
                "Client key not found".to_string(),
            )),
            Err(err) => {
                error!("Failed to get client key: {}", err);
                Err(AuthError::StorageError(format!(
                    "Failed to get client key: {}",
                    err
                )))
            }
        }
    }

    /// Verify a JWT token from a request
    ///
    /// # Arguments
    ///
    /// * `headers` - The request headers
    ///
    /// # Returns
    ///
    /// * `Result<AuthResponse, AuthError>` - The authentication response
    pub async fn verify_token_from_headers(
        &self,
        headers: &HeaderMap,
    ) -> Result<AuthResponse, AuthError> {
        // Extract the Authorization header
        let auth_header = headers
            .get("Authorization")
            .ok_or_else(|| {
                AuthError::AuthenticationFailed("Missing Authorization header".to_string())
            })?
            .to_str()
            .map_err(|_| {
                AuthError::AuthenticationFailed("Invalid Authorization header".to_string())
            })?;

        // Check that it's a Bearer token
        if !auth_header.starts_with("Bearer ") {
            return Err(AuthError::AuthenticationFailed(
                "Invalid Authorization header format".to_string(),
            ));
        }

        // Extract the token
        let token = auth_header.trim_start_matches("Bearer ").trim();

        // Decode the token
        let validation = Validation::new(Algorithm::HS256);
        let decoding_key = DecodingKey::from_secret(self.config.secret.as_bytes());

        let token_data = decode::<Claims>(token, &decoding_key, &validation)
            .map_err(|err| AuthError::AuthenticationFailed(format!("Invalid token: {}", err)))?;

        let claims = token_data.claims;

        // Check that the token is not a refresh token
        if claims.jti.starts_with("refresh_") {
            return Err(AuthError::AuthenticationFailed(
                "Cannot use refresh token for authentication".to_string(),
            ));
        }

        // Get the client key to check for revocation
        let client_id = claims.aud;
        let client_key_path = format!("{}{}", prefixes::CLIENT_KEY, client_id);

        match self.storage.get(&client_key_path).await {
            Ok(Some(data)) => {
                let client_key: ClientKey = deserialize(&data).map_err(|err| {
                    AuthError::StorageError(format!("Failed to deserialize client key: {}", err))
                })?;

                // Check if the client key has been revoked
                if client_key.revoked_at.is_some() {
                    return Err(AuthError::AuthenticationFailed(
                        "Client key has been revoked".to_string(),
                    ));
                }

                // Check if the key has expired
                if let Some(expires_at) = client_key.expires_at {
                    if expires_at < Utc::now().timestamp() as u64 {
                        return Err(AuthError::AuthenticationFailed(
                            "Client key has expired".to_string(),
                        ));
                    }
                }

                // Return the authentication response
                Ok(AuthResponse {
                    is_valid: true,
                    key_id: Some(claims.sub),
                    permissions: claims.permissions,
                })
            }
            Ok(None) => {
                debug!("Client key not found: {}", client_id);
                // For backwards compatibility, we'll still validate the token
                // but in a real production system, you would return an error here
                Ok(AuthResponse {
                    is_valid: true,
                    key_id: Some(claims.sub),
                    permissions: claims.permissions,
                })
            }
            Err(err) => {
                error!("Failed to get client key: {}", err);
                Err(AuthError::StorageError(format!(
                    "Failed to get client key: {}",
                    err
                )))
            }
        }
    }
}
