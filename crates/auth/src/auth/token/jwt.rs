use std::sync::Arc;

use axum::http::HeaderMap;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid;

use crate::config::JwtConfig;
use crate::secrets::SecretManager;
use crate::storage::{KeyManager, Storage};
use crate::{AuthError, AuthResponse};

/// Token type enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenType {
    Access,
    Refresh,
}

/// JWT Claims structure
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user ID - public key for root, client id for client key)
    pub sub: String,
    /// Issuer
    pub iss: String,
    /// Audience (client ID)
    pub aud: String,
    /// Expiration time (as Unix timestamp)
    pub exp: u64,
    /// Issued at (as Unix timestamp)
    pub iat: u64,
    /// JWT ID
    pub jti: String,
    /// Permissions (context[<context-id>, <user-id>] format for client keys)
    pub permissions: Vec<String>,
}

/// JWT Token Manager
///
/// This component handles JWT token generation and verification.
#[derive(Clone)]
pub struct TokenManager {
    config: JwtConfig,
    storage: Arc<dyn Storage>,
    key_manager: KeyManager,
    secret_manager: Arc<SecretManager>,
}

impl TokenManager {
    /// Create a new JWT token manager
    ///
    /// # Arguments
    ///
    /// * `config` - JWT configuration
    /// * `storage` - Storage backend
    /// * `secret_manager` - Secret manager
    ///
    /// # Returns
    ///
    /// * `Self` - The token generator
    pub fn new(
        config: JwtConfig,
        storage: Arc<dyn Storage>,
        secret_manager: Arc<SecretManager>,
    ) -> Self {
        let key_manager = KeyManager::new(Arc::clone(&storage));
        Self {
            config,
            storage,
            key_manager,
            secret_manager,
        }
    }

    /// Generate a JWT token
    async fn generate_token(
        &self,
        user_id: String,
        permissions: Vec<String>,
        expiry: Duration,
    ) -> Result<String, AuthError> {
        let now = Utc::now();
        let exp = now + expiry;

        let claims = Claims {
            sub: user_id.clone(),
            iss: self.config.issuer.clone(),
            aud: user_id,
            exp: exp.timestamp() as u64,
            iat: now.timestamp() as u64,
            jti: uuid::Uuid::new_v4().to_string(),
            permissions,
        };

        let secret = self
            .secret_manager
            .get_jwt_auth_secret()
            .await
            .map_err(|e| AuthError::TokenGenerationFailed(e.to_string()))?;

        let header = Header::new(Algorithm::HS256);
        encode(
            &header,
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .map_err(|e| AuthError::TokenGenerationFailed(e.to_string()))
    }

    /// Generate a token pair
    pub async fn generate_token_pair(
        &self,
        user_id: String,
        permissions: Vec<String>,
    ) -> Result<(String, String), AuthError> {
        let access_token = self
            .generate_token(
                user_id.clone(),
                permissions.clone(),
                Duration::seconds(self.config.access_token_expiry as i64),
            )
            .await?;

        let refresh_token = self
            .generate_token(
                user_id,
                permissions,
                Duration::seconds(self.config.refresh_token_expiry as i64),
            )
            .await?;

        Ok((access_token, refresh_token))
    }

    /// Verify a JWT token and return the claims
    pub async fn verify_token(&self, token: &str) -> Result<Claims, AuthError> {
        let secret = self
            .secret_manager
            .get_jwt_auth_secret()
            .await
            .map_err(|e| AuthError::TokenGenerationFailed(e.to_string()))?;

        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = true;
        validation.set_issuer(&[&self.config.issuer]);

        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(secret.as_bytes()),
            &validation,
        )
        .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        Ok(token_data.claims)
    }

    /// Verify a JWT token from request headers
    pub async fn verify_token_from_headers(
        &self,
        headers: &HeaderMap,
    ) -> Result<AuthResponse, AuthError> {
        let auth_header = headers
            .get("Authorization")
            .ok_or_else(|| AuthError::InvalidRequest("Missing Authorization header".to_string()))?
            .to_str()
            .map_err(|e| {
                AuthError::InvalidRequest(format!("Invalid Authorization header: {}", e))
            })?;

        if !auth_header.starts_with("Bearer ") {
            return Err(AuthError::InvalidRequest(
                "Invalid Authorization header format. Expected 'Bearer <token>'".to_string(),
            ));
        }

        let token = auth_header.trim_start_matches("Bearer ").trim();
        if token.is_empty() {
            return Err(AuthError::InvalidRequest(
                "Empty token provided".to_string(),
            ));
        }

        let claims = self.verify_token(token).await?;

        Ok(AuthResponse {
            is_valid: true,
            key_id: claims.sub,
            permissions: claims.permissions,
        })
    }

    /// Revoke a client's tokens
    ///
    /// # Arguments
    ///
    /// * `client_id` - The client ID to revoke
    ///
    /// # Returns
    ///
    /// * `Result<(), AuthError>` - Success or error
    pub async fn revoke_client_tokens(&self, client_id: &str) -> Result<(), AuthError> {
        let client_key = self
            .key_manager
            .get_client_key(client_id)
            .await
            .map_err(|e| AuthError::StorageError(e.to_string()))?
            .ok_or_else(|| AuthError::InvalidToken("Client key not found".to_string()))?;

        // Revoke the client key
        let mut client_key = client_key.clone();
        client_key.revoke();

        // Save the updated client key using KeyManager
        self.key_manager
            .set_client_key(client_id, &client_key)
            .await
            .map_err(|e| AuthError::StorageError(format!("Failed to update client key: {}", e)))?;

        Ok(())
    }

    /// Refresh a token pair using a refresh token
    ///
    /// # Arguments
    ///
    /// * `refresh_token` - The refresh token to use
    ///
    /// # Returns
    ///
    /// * `Result<(String, String), AuthError>` - New access and refresh tokens
    pub async fn refresh_token_pair(
        &self,
        refresh_token: &str,
    ) -> Result<(String, String), AuthError> {
        // Verify the refresh token and get claims
        let claims = self.verify_token(refresh_token).await?;

        // Check if this is a root key or client key based on ID format
        // Client keys follow the pattern "client_<context_id>_<context_identity>"
        let is_root_key = !claims.sub.starts_with("client_");

        if is_root_key {
            // For root key, verify it exists and is valid
            let root_key = self
                .key_manager
                .get_root_key(&claims.sub)
                .await
                .map_err(|e| AuthError::StorageError(e.to_string()))?
                .ok_or_else(|| AuthError::InvalidToken("Root key not found".to_string()))?;

            if !root_key.is_valid() {
                return Err(AuthError::InvalidToken(
                    "Root key has been revoked".to_string(),
                ));
            }
        } else {
            // For client key, verify it exists and is valid
            let client_key = self
                .key_manager
                .get_client_key(&claims.sub)
                .await
                .map_err(|e| AuthError::StorageError(e.to_string()))?
                .ok_or_else(|| AuthError::InvalidToken("Client key not found".to_string()))?;

            if !client_key.is_valid() {
                return Err(AuthError::InvalidToken(
                    "Client key has been revoked".to_string(),
                ));
            }
        }

        // Generate new token pair with the same permissions
        self.generate_token_pair(claims.sub, claims.permissions)
            .await
    }
}
