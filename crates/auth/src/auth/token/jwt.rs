use std::sync::Arc;

use axum::http::HeaderMap;
use base64::Engine;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand::Rng;
use serde::{Deserialize, Serialize};
use {base64, rand, uuid};

use crate::api::handlers::auth::ChallengeResponse;
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
    /// Subject (key ID)
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
}

/// Challenge Claims structure
#[derive(Debug, Serialize, Deserialize)]
pub struct ChallengeClaims {
    /// Issuer of the challenge
    pub iss: String,
    /// Unique challenge ID
    pub jti: String,
    /// Timestamp when the challenge was issued
    pub iat: u64,
    /// Expiration time of the challenge
    pub exp: u64,
    /// Nonce
    pub nonce: String,
}

/// JWT Token Manager
///
/// This component handles JWT token generation and verification.
#[derive(Clone)]
pub struct TokenManager {
    config: JwtConfig,
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
            key_manager,
            secret_manager,
        }
    }

    /// Generate a JWT token
    async fn generate_token(
        &self,
        key_id: String,
        permissions: Vec<String>,
        expiry: Duration,
    ) -> Result<String, AuthError> {
        let now = Utc::now();
        let exp = now + expiry;

        let claims = Claims {
            sub: key_id.clone(),
            iss: self.config.issuer.clone(),
            aud: self.config.issuer.clone(),
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
        key_id: String,
        permissions: Vec<String>,
    ) -> Result<(String, String), AuthError> {
        // Verify the key exists and is valid
        let key = self
            .key_manager
            .get_key(&key_id)
            .await
            .map_err(|e| AuthError::StorageError(e.to_string()))?
            .ok_or_else(|| AuthError::InvalidToken("Key not found".to_string()))?;

        if !key.is_valid() {
            return Err(AuthError::InvalidToken("Key has been revoked".to_string()));
        }

        let access_token = self
            .generate_token(
                key_id.clone(),
                permissions.clone(),
                Duration::seconds(self.config.access_token_expiry as i64),
            )
            .await?;

        let refresh_token = self
            .generate_token(
                key_id,
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
        validation.set_audience(&[&self.config.issuer]);

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

        // Verify the key exists and is valid
        let key = self
            .key_manager
            .get_key(&claims.sub)
            .await
            .map_err(|e| AuthError::StorageError(e.to_string()))?
            .ok_or_else(|| AuthError::InvalidToken("Key not found".to_string()))?;

        if !key.is_valid() {
            return Err(AuthError::InvalidToken("Key has been revoked".to_string()));
        }

        Ok(AuthResponse {
            is_valid: true,
            key_id: claims.sub,
            permissions: claims.permissions,
        })
    }

    /// Revoke a key's tokens
    ///
    /// # Arguments
    ///
    /// * `key_id` - The key ID to revoke
    ///
    /// # Returns
    ///
    /// * `Result<(), AuthError>` - Success or error
    pub async fn revoke_client_tokens(&self, key_id: &str) -> Result<(), AuthError> {
        // Get the key
        let mut key = self
            .key_manager
            .get_key(key_id)
            .await
            .map_err(|e| AuthError::StorageError(e.to_string()))?
            .ok_or_else(|| AuthError::InvalidToken("Key not found".to_string()))?;

        // Revoke the key
        key.revoke();

        // Save the updated key
        self.key_manager
            .set_key(key_id, &key)
            .await
            .map_err(|e| AuthError::StorageError(format!("Failed to update key: {}", e)))?;

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
        // Verify the refresh token to get the claims
        let claims = self.verify_token(refresh_token).await?;

        // Get and verify the key
        let key = self
            .key_manager
            .get_key(&claims.sub)
            .await
            .map_err(|e| AuthError::StorageError(e.to_string()))?
            .ok_or_else(|| AuthError::InvalidToken("Key not found".to_string()))?;

        if !key.is_valid() {
            return Err(AuthError::InvalidToken("Key has been revoked".to_string()));
        }

        // Generate new token pair with the same permissions
        self.generate_token_pair(claims.sub, claims.permissions)
            .await
    }

    /// Generate a challenge token
    ///
    /// # Returns
    ///
    /// * `Result<ChallengeResponse, AuthError>` - The generated challenge token and nonce
    pub async fn generate_challenge(&self) -> Result<ChallengeResponse, AuthError> {
        let now = Utc::now();
        // Challenges should be short-lived, using a 5-minute expiry
        let exp = now + Duration::minutes(5);

        // Generate a secure random nonce
        let mut nonce_bytes = [0u8; 32];
        rand::thread_rng().try_fill(&mut nonce_bytes).map_err(|e| {
            AuthError::TokenGenerationFailed(format!("Failed to generate nonce: {}", e))
        })?;
        let nonce = base64::engine::general_purpose::STANDARD.encode(nonce_bytes);

        let claims = ChallengeClaims {
            iss: self.config.issuer.clone(),
            jti: uuid::Uuid::new_v4().to_string(),
            iat: now.timestamp() as u64,
            exp: exp.timestamp() as u64,
            nonce: nonce.clone(),
        };

        let secret = self
            .secret_manager
            .get_jwt_challenge_secret()
            .await
            .map_err(|e| AuthError::TokenGenerationFailed(e.to_string()))?;

        let header = Header::new(Algorithm::HS256);
        let challenge = encode(
            &header,
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .map_err(|e| AuthError::TokenGenerationFailed(e.to_string()))?;

        Ok(ChallengeResponse { challenge, nonce })
    }

    /// Verify a challenge token
    ///
    /// # Arguments
    ///
    /// * `token` - The challenge token to verify
    ///
    /// # Returns
    ///
    /// * `Result<ChallengeClaims, AuthError>` - The verified challenge claims
    pub async fn verify_challenge(&self, token: &str) -> Result<ChallengeClaims, AuthError> {
        let secret = self
            .secret_manager
            .get_jwt_challenge_secret()
            .await
            .map_err(|e| AuthError::TokenGenerationFailed(e.to_string()))?;

        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = true;
        validation.set_issuer(&[&self.config.issuer]);

        let token_data = decode::<ChallengeClaims>(
            token,
            &DecodingKey::from_secret(secret.as_bytes()),
            &validation,
        )
        .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        Ok(token_data.claims)
    }
}
