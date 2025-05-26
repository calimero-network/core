use std::sync::Arc;

use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use tracing::{debug, error};
use uuid;

use crate::config::JwtConfig;
use crate::secrets::SecretManager;
use crate::storage::models::ClientKey;
use crate::storage::{Storage, KeyManager};
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
    /// Subject (user ID)
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
    /// Permissions
    pub permissions: Vec<String>,
    /// Token type
    #[serde(rename = "typ")]
    pub token_type: String,
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

    /// Generate a JWT token with specified type
    async fn generate_token_internal(
        &self,
        client_key: &ClientKey,
        token_type: TokenType,
        permissions: Option<Vec<String>>,
    ) -> Result<String, AuthError> {
        let now = Utc::now();
        let exp = now
            + match token_type {
                TokenType::Access => Duration::seconds(self.config.access_token_expiry as i64),
                TokenType::Refresh => Duration::seconds(self.config.refresh_token_expiry as i64),
            };

        let jwt_id = match token_type {
            TokenType::Access => uuid::Uuid::new_v4().to_string(),
            TokenType::Refresh => format!("refresh_{}_{}", client_key.client_id, now.timestamp()),
        };

        let claims = Claims {
            sub: client_key.client_id.clone(),
            iss: self.config.issuer.clone(),
            aud: client_key.client_id.clone(),
            exp: exp.timestamp() as u64,
            iat: now.timestamp() as u64,
            jti: jwt_id,
            permissions: permissions.unwrap_or_else(|| match token_type {
                TokenType::Access => client_key.permissions.clone(),
                TokenType::Refresh => vec![],
            }),
            token_type: match token_type {
                TokenType::Access => "access".to_string(),
                TokenType::Refresh => "refresh".to_string(),
            },
        };

        let secret = self
            .secret_manager
            .get_secret()
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

    /// Generate an access token
    pub async fn generate_access_token(&self, client_key: &ClientKey) -> Result<String, AuthError> {
        self.generate_token_internal(client_key, TokenType::Access, None)
            .await
    }

    /// Generate a refresh token
    async fn generate_refresh_token(&self, client_key: &ClientKey) -> Result<String, AuthError> {
        self.generate_token_internal(client_key, TokenType::Refresh, None)
            .await
    }

    /// Verify a JWT token and return the claims
    pub async fn verify_token(
        &self,
        token: &str,
        expected_type: Option<TokenType>,
    ) -> Result<Claims, AuthError> {
        let secret = self
            .secret_manager
            .get_secret()
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

        let claims = token_data.claims;

        // Validate token type if specified
        if let Some(expected) = expected_type {
            let token_type = match claims.token_type.as_str() {
                "access" => TokenType::Access,
                "refresh" => TokenType::Refresh,
                _ => return Err(AuthError::InvalidToken("Invalid token type".to_string())),
            };
            if token_type != expected {
                return Err(AuthError::InvalidToken("Incorrect token type".to_string()));
            }
        }

        // Get the client key to verify it's still valid
        let client_key = self
            .key_manager
            .get_client_key(&claims.sub)
            .await
            .map_err(|e| AuthError::StorageError(e.to_string()))?
            .ok_or_else(|| AuthError::InvalidToken("Client key not found".to_string()))?;

        // Check if the key is revoked
        if client_key.is_revoked() {
            return Err(AuthError::InvalidToken("Client key is revoked".to_string()));
        }

        // Validate audience
        if claims.aud != client_key.client_id {
            return Err(AuthError::InvalidToken(
                "Invalid token audience".to_string(),
            ));
        }

        // Update last used timestamp
        let mut client_key = client_key.clone();
        client_key.update_last_used();
        self.key_manager
            .set_client_key(&client_key.client_id, &client_key)
            .await
            .map_err(|e| AuthError::StorageError(e.to_string()))?;

        Ok(claims)
    }

    /// Generate an access token and refresh token pair
    ///
    /// # Arguments
    ///
    /// * `client_key` - The client key to generate tokens for
    ///
    /// # Returns
    ///
    /// * `Result<(String, String), AuthError>` - The generated access and refresh tokens
    pub async fn generate_token_pair(
        &self,
        client_key: &ClientKey,
    ) -> Result<(String, String), AuthError> {
        if !client_key.is_valid() {
            return Err(AuthError::InvalidToken(
                "Client key is not valid".to_string(),
            ));
        }

        let access_token = self.generate_access_token(client_key).await?;
        let refresh_token = self.generate_refresh_token(client_key).await?;

        Ok((access_token, refresh_token))
    }

    /// Refresh a token pair using a refresh token
    pub async fn refresh_token_pair(
        &self,
        refresh_token: &str,
    ) -> Result<(String, String), AuthError> {
        // Verify the refresh token specifically
        let claims = self
            .verify_token(refresh_token, Some(TokenType::Refresh))
            .await?;

        // Get the client key
        let client_key = self
            .key_manager
            .get_client_key(&claims.aud)
            .await
            .map_err(|e| AuthError::StorageError(e.to_string()))?
            .ok_or_else(|| AuthError::InvalidToken("Client key not found".to_string()))?;

        // Generate new token pair
        self.generate_token_pair(&client_key).await
    }

    /// Verify a JWT token from request headers
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
        headers: &axum::http::HeaderMap,
    ) -> Result<AuthResponse, AuthError> {
        // Extract the Authorization header
        let auth_header = headers
            .get("Authorization")
            .ok_or_else(|| AuthError::InvalidRequest("Missing Authorization header".to_string()))?
            .to_str()
            .map_err(|e| {
                AuthError::InvalidRequest(format!("Invalid Authorization header: {}", e))
            })?;

        // Check that it's a Bearer token
        if !auth_header.starts_with("Bearer ") {
            return Err(AuthError::InvalidRequest(
                "Invalid Authorization header format. Expected 'Bearer <token>'".to_string(),
            ));
        }

        // Extract the token
        let token = auth_header.trim_start_matches("Bearer ").trim();
        if token.is_empty() {
            return Err(AuthError::InvalidRequest(
                "Empty token provided".to_string(),
            ));
        }

        // Verify the token and get claims (must be an access token)
        let claims = self.verify_token(token, Some(TokenType::Access)).await?;

        Ok(AuthResponse {
            is_valid: true,
            key_id: Some(claims.sub),
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
}
