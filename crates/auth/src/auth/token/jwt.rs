use std::sync::Arc;

use axum::http::HeaderMap;
use base64::Engine;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use {base64, hex, rand, uuid};

use crate::api::handlers::auth::ChallengeResponse;
use crate::config::JwtConfig;
use crate::secrets::SecretManager;
use crate::storage::models::KeyType;
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
    /// Node URL this token is valid for (optional, for backward compatibility)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_url: Option<String>,
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

    /// Validate that a token's node_url matches the request host
    ///
    /// This function compares the host from the token's node_url with the original
    /// host from the request headers. It skips validation for internal auth service
    /// requests.
    ///
    /// # Arguments
    ///
    /// * `token_node_url` - The node URL from the JWT token
    /// * `headers` - The request headers
    ///
    /// # Returns
    ///
    /// * `Result<(), String>` - Ok if validation passes, Err with error message if not
    pub fn validate_node_host(
        &self,
        token_node_url: &str,
        headers: &HeaderMap,
    ) -> Result<(), String> {
        let request_host = headers
            .get("X-Forwarded-Host")
            .or_else(|| headers.get("host"))
            .and_then(|h| h.to_str().ok());

        if let Some(request_host) = request_host {
            // Skip validation if request is coming from internal auth service.
            // Only allow exact "auth" hostname or "auth:" followed by a numeric port.
            // This prevents bypass attacks using forged headers like "auth:malicious.com".
            if Self::is_internal_auth_service(request_host) {
                return Ok(());
            }

            // Extract the host from the token's node URL
            if let Ok(token_url) = Url::parse(token_node_url) {
                if let Some(token_host) = token_url.host_str() {
                    // Compare the hosts (handle both with and without port)
                    let request_host_without_port =
                        request_host.split(':').next().unwrap_or(request_host);
                    if request_host_without_port != token_host && request_host != token_host {
                        return Err(format!(
                            "Token is not valid for this host. Token is for '{token_host}' but request is to '{request_host}'"
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Check if the host represents an internal auth service.
    ///
    /// This is a robust check that only matches:
    /// - Exact "auth" hostname
    /// - "auth:" followed by a valid numeric port (e.g., "auth:3001")
    ///
    /// This prevents bypass attacks where an attacker forges headers like
    /// "auth:malicious.com" to skip validation.
    fn is_internal_auth_service(host: &str) -> bool {
        if host == "auth" {
            return true;
        }

        if let Some(port_str) = host.strip_prefix("auth:") {
            // Port must be non-empty and contain only digits
            return !port_str.is_empty() && port_str.chars().all(|c| c.is_ascii_digit());
        }

        false
    }

    /// Generate a JWT token
    async fn generate_token(
        &self,
        key_id: String,
        permissions: Vec<String>,
        expiry: Duration,
        node_url: Option<String>,
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
            node_url,
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

    /// Generate a pair of JWT tokens without key validation.
    async fn generate_raw_token_pair(
        &self,
        key_id: String,
        permissions: Vec<String>,
        node_url: Option<String>,
        access_expiry: Duration,
        refresh_expiry: Duration,
    ) -> Result<(String, String), AuthError> {
        let access_token = self
            .generate_token(
                key_id.clone(),
                permissions.clone(),
                access_expiry,
                node_url.clone(),
            )
            .await?;

        let refresh_token = self
            .generate_token(key_id, permissions, refresh_expiry, node_url)
            .await?;

        Ok((access_token, refresh_token))
    }

    /// Generate mock tokens without requiring key storage (for CI/testing only)
    ///
    /// This method bypasses all key storage and validation for mock token generation.
    /// Should only be used in development/testing environments.
    pub async fn generate_mock_token_pair(
        &self,
        key_id: String,
        permissions: Vec<String>,
        node_url: Option<String>,
        custom_expiry: Option<u64>,
    ) -> Result<(String, String), AuthError> {
        let access_expiry =
            Duration::seconds(custom_expiry.unwrap_or(self.config.access_token_expiry) as i64);
        let refresh_expiry = Duration::seconds(self.config.refresh_token_expiry as i64);

        self.generate_raw_token_pair(key_id, permissions, node_url, access_expiry, refresh_expiry)
            .await
    }

    /// Generate a pair of access and refresh tokens
    ///
    /// # Arguments
    ///
    /// * `key_id` - The key ID
    /// * `permissions` - The permissions to include in the token
    /// * `node_url` - The node URL this token is valid for (optional)
    ///
    /// # Returns
    ///
    /// * `Result<(String, String), AuthError>` - The access and refresh tokens
    pub async fn generate_token_pair(
        &self,
        key_id: String,
        permissions: Vec<String>,
        node_url: Option<String>,
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

        let access_expiry = Duration::seconds(self.config.access_token_expiry as i64);
        let refresh_expiry = Duration::seconds(self.config.refresh_token_expiry as i64);

        match key.key_type {
            // For root tokens, simply generate new tokens with the same ID
            KeyType::Root => {
                self.generate_raw_token_pair(
                    key_id,
                    permissions,
                    node_url,
                    access_expiry,
                    refresh_expiry,
                )
                .await
            }
            // For client tokens, use the same key ID - no rotation during initial generation
            KeyType::Client => {
                self.generate_raw_token_pair(
                    key_id,
                    permissions,
                    node_url,
                    access_expiry,
                    refresh_expiry,
                )
                .await
            }
        }
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

        match decode::<Claims>(
            token,
            &DecodingKey::from_secret(secret.as_bytes()),
            &validation,
        ) {
            Ok(token_data) => Ok(token_data.claims),
            Err(err) => match err.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => {
                    Err(AuthError::InvalidToken("Token has expired".to_string()))
                }
                jsonwebtoken::errors::ErrorKind::InvalidToken => {
                    Err(AuthError::InvalidToken(format!("Malformed token: {err}")))
                }
                _ => Err(AuthError::InvalidToken(err.to_string())),
            },
        }
    }

    async fn touch_key_last_activity(&self, key_id: &str) -> Result<(), AuthError> {
        // Re-fetch key to avoid overwriting revocations with stale data.
        let Some(mut key) = self
            .key_manager
            .get_key(key_id)
            .await
            .map_err(|e| AuthError::StorageError(e.to_string()))?
        else {
            return Ok(());
        };

        key.metadata.touch();
        self.key_manager
            .set_key(key_id, &key)
            .await
            .map_err(|e| AuthError::StorageError(e.to_string()))?;

        Ok(())
    }

    /// Verify a JWT token from request headers
    ///
    /// This method validates the token, checks for idle timeout, and updates the
    /// last activity timestamp to implement sliding window session management.
    pub async fn verify_token_from_headers(
        &self,
        headers: &HeaderMap,
    ) -> Result<AuthResponse, AuthError> {
        let auth_header = headers
            .get("Authorization")
            .ok_or_else(|| AuthError::InvalidRequest("Missing Authorization header".to_string()))?
            .to_str()
            .map_err(|e| AuthError::InvalidRequest(format!("Invalid Authorization header: {e}")))?;

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

        // Check node URL if token has node information
        if let Some(token_node_url) = &claims.node_url {
            if let Err(error_msg) = self.validate_node_host(token_node_url, headers) {
                return Err(AuthError::InvalidToken(error_msg));
            }
        }

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

        // Check for idle timeout - if the session has been inactive for too long, reject it
        if key.metadata.is_idle(self.config.idle_timeout) {
            tracing::debug!(
                "Session for key {} has exceeded idle timeout of {} seconds",
                claims.sub,
                self.config.idle_timeout
            );
            return Err(AuthError::InvalidToken(
                "Session has expired due to inactivity".to_string(),
            ));
        }

        // Update last activity timestamp (sliding window expiration)
        if let Err(e) = self.touch_key_last_activity(&claims.sub).await {
            // Log the error but don't fail the request - activity tracking is best-effort
            tracing::warn!(
                "Failed to update last activity for key {}: {}",
                claims.sub,
                e
            );
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
            .map_err(|e| AuthError::StorageError(format!("Failed to update key: {e}")))?;

        Ok(())
    }

    /// Refresh a token pair using a refresh token
    ///
    /// This method verifies the refresh token and generates new tokens based on the key type.
    /// For root tokens, it preserves the key ID.
    /// For client tokens, it generates a new client ID and rotates the key.
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

        // Get the key and verify it's valid
        let mut key = self
            .key_manager
            .get_key(&claims.sub)
            .await
            .map_err(|e| {
                tracing::error!("Storage error while getting key {}: {}", claims.sub, e);
                AuthError::StorageError(e.to_string())
            })?
            .ok_or_else(|| {
                tracing::error!("Key not found: {}", claims.sub);
                AuthError::InvalidToken(format!("Key not found: {}", claims.sub))
            })?;

        if !key.is_valid() {
            return Err(AuthError::InvalidToken("Key is not valid".to_string()));
        }

        // Check for idle timeout - if the session has been inactive for too long, reject it
        if key.metadata.is_idle(self.config.idle_timeout) {
            tracing::debug!(
                "Session for key {} has exceeded idle timeout of {} seconds",
                claims.sub,
                self.config.idle_timeout
            );
            return Err(AuthError::InvalidToken(
                "Session has expired due to inactivity".to_string(),
            ));
        }

        // Update last activity timestamp (sliding window expiration)
        if let Err(e) = self.touch_key_last_activity(&claims.sub).await {
            // Log the error but don't fail the refresh - activity tracking is best-effort
            tracing::warn!(
                "Failed to update last activity for key {}: {}",
                claims.sub,
                e
            );
        } else {
            // Keep the in-memory key in sync for client rotation.
            key.metadata.touch();
        }

        match key.key_type {
            // For root tokens, simply generate new tokens with the same ID
            KeyType::Root => {
                self.generate_token_pair(claims.sub, key.permissions, claims.node_url.clone())
                    .await
            }
            // For client tokens, rotate the key ID
            KeyType::Client => {
                // Generate new client ID
                let timestamp = Utc::now().timestamp();
                let mut hasher = Sha256::new();
                hasher.update(format!("refresh:{}:{}", claims.sub, timestamp).as_bytes());
                let new_client_id = hex::encode(hasher.finalize());

                tracing::debug!(
                    "Rotating client key from {} to {}",
                    claims.sub,
                    new_client_id
                );

                // Generate tokens FIRST, before any key mutations.
                // This ensures that if token generation fails, we haven't modified any keys
                // and the user's original key remains valid (no lockout scenario).
                let access_expiry = Duration::seconds(self.config.access_token_expiry as i64);
                let refresh_expiry = Duration::seconds(self.config.refresh_token_expiry as i64);

                let (access_token, refresh_token) = self
                    .generate_raw_token_pair(
                        new_client_id.clone(),
                        key.permissions.clone(),
                        claims.node_url.clone(),
                        access_expiry,
                        refresh_expiry,
                    )
                    .await?;

                // Tokens generated successfully - now perform key rotation.
                // Store the new key first to ensure we don't lose access.
                if let Err(e) = self.key_manager.set_key(&new_client_id, &key).await {
                    tracing::error!(
                        "Failed to store new client key {} during rotation: {}",
                        new_client_id,
                        e
                    );
                    // Token generation succeeded but key storage failed.
                    // Return error - user's old key is still valid, so no lockout.
                    return Err(AuthError::StorageError(format!(
                        "Failed to store new client key during rotation: {e}"
                    )));
                }

                tracing::debug!("Successfully stored new client key: {}", new_client_id);

                // Now safely delete the old key
                if let Err(e) = self.key_manager.delete_key(&claims.sub).await {
                    // Log the error but don't fail the refresh - tokens are already generated
                    // and new key is stored. User can use the new tokens.
                    tracing::warn!(
                        "Failed to delete old client key {} after successful rotation: {}",
                        claims.sub,
                        e
                    );
                }

                Ok((access_token, refresh_token))
            }
        }
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
            AuthError::TokenGenerationFailed(format!("Failed to generate nonce: {e}"))
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

    /// Get the key manager
    pub fn get_key_manager(&self) -> &KeyManager {
        &self.key_manager
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_internal_auth_service_valid_cases() {
        // Exact "auth" hostname should be valid
        assert!(TokenManager::is_internal_auth_service("auth"));

        // "auth:" followed by numeric port should be valid
        assert!(TokenManager::is_internal_auth_service("auth:3001"));
        assert!(TokenManager::is_internal_auth_service("auth:80"));
        assert!(TokenManager::is_internal_auth_service("auth:443"));
        assert!(TokenManager::is_internal_auth_service("auth:8080"));
        assert!(TokenManager::is_internal_auth_service("auth:1"));
        assert!(TokenManager::is_internal_auth_service("auth:65535"));
    }

    #[test]
    fn test_is_internal_auth_service_bypass_attempts() {
        // These are potential bypass attempts that should be rejected

        // Attacker trying to bypass with malicious domain
        assert!(!TokenManager::is_internal_auth_service(
            "auth:malicious.com"
        ));
        assert!(!TokenManager::is_internal_auth_service(
            "auth:evil.example.com"
        ));

        // Attacker trying to use non-numeric port
        assert!(!TokenManager::is_internal_auth_service("auth:abc"));
        assert!(!TokenManager::is_internal_auth_service("auth:80abc"));
        assert!(!TokenManager::is_internal_auth_service("auth:abc80"));
        assert!(!TokenManager::is_internal_auth_service("auth:80.0"));
        assert!(!TokenManager::is_internal_auth_service("auth:80:80"));

        // Empty port should be rejected
        assert!(!TokenManager::is_internal_auth_service("auth:"));

        // Similar prefixes that should not match
        assert!(!TokenManager::is_internal_auth_service("authentication"));
        assert!(!TokenManager::is_internal_auth_service("auth-service"));
        assert!(!TokenManager::is_internal_auth_service("auth_service"));
        assert!(!TokenManager::is_internal_auth_service("authservice:3001"));

        // Other hostnames should not match
        assert!(!TokenManager::is_internal_auth_service("localhost"));
        assert!(!TokenManager::is_internal_auth_service("localhost:3001"));
        assert!(!TokenManager::is_internal_auth_service("example.com"));
    }

    #[test]
    fn test_is_internal_auth_service_edge_cases() {
        // Case sensitivity - "auth" should be lowercase only
        assert!(!TokenManager::is_internal_auth_service("AUTH"));
        assert!(!TokenManager::is_internal_auth_service("Auth"));
        assert!(!TokenManager::is_internal_auth_service("AUTH:3001"));

        // Whitespace variations
        assert!(!TokenManager::is_internal_auth_service(" auth"));
        assert!(!TokenManager::is_internal_auth_service("auth "));
        assert!(!TokenManager::is_internal_auth_service(" auth:3001"));

        // Special characters in port
        assert!(!TokenManager::is_internal_auth_service("auth:-3001"));
        assert!(!TokenManager::is_internal_auth_service("auth:+3001"));
        assert!(!TokenManager::is_internal_auth_service("auth: 3001"));
    }
}
