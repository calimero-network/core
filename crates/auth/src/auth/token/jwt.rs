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
use crate::secrets::{SecretManager, SecretType};
use crate::storage::models::KeyType;
use crate::storage::{KeyManager, Storage};
use crate::{AuthError, AuthResponse};

/// Token type enum.
///
/// Serialized on the wire as a stable lowercase string (`"access"` /
/// `"refresh"`) inside the JWT `token_type` claim. This distinguishes an
/// access credential from a refresh credential so that a long-lived refresh
/// token can no longer be replayed as a bearer access token (finding #1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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
    /// Token type — distinguishes access from refresh tokens and is enforced
    /// during verification. This is a required claim: tokens minted before this
    /// field existed are intentionally rejected (clean break, finding #1).
    pub token_type: TokenType,
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
        token_type: TokenType,
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
            token_type,
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
                TokenType::Access,
            )
            .await?;

        let refresh_token = self
            .generate_token(
                key_id,
                permissions,
                refresh_expiry,
                node_url,
                TokenType::Refresh,
            )
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

    /// Decode and signature-verify a JWT, returning its raw claims.
    ///
    /// Verification is attempted against the current primary JWT secret and, on
    /// a signature mismatch, against any backup secret still inside its grace
    /// window (finding #5). Without this fallback, every outstanding token would
    /// fail to verify the instant a secret rotated, logging out the whole fleet.
    /// All non-signature checks (issuer, audience, malformed token) are terminal.
    ///
    /// `validate_exp` controls expiry enforcement: callers that must inspect the
    /// claims of an already-expired token (the refresh endpoint binding an
    /// expired access token to its refresh token, finding #3) pass `false`.
    /// This performs **no** `token_type` check — that is layered on by the typed
    /// wrappers below.
    async fn decode_with_secrets(
        &self,
        token: &str,
        validate_exp: bool,
    ) -> Result<Claims, AuthError> {
        let secrets = self
            .secret_manager
            .get_verify_secrets(SecretType::JwtAuth)
            .await
            .map_err(|e| AuthError::TokenGenerationFailed(e.to_string()))?;

        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = validate_exp;
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_audience(&[&self.config.issuer]);

        // Retried only on signature mismatch; populated with the last such error
        // so the final message matches the legacy single-secret behaviour.
        let mut last_signature_err: Option<jsonwebtoken::errors::Error> = None;

        for secret in &secrets {
            match decode::<Claims>(
                token,
                &DecodingKey::from_secret(secret.as_bytes()),
                &validation,
            ) {
                Ok(token_data) => return Ok(token_data.claims),
                Err(err) => match err.kind() {
                    // A different signing secret might still validate this token.
                    jsonwebtoken::errors::ErrorKind::InvalidSignature => {
                        last_signature_err = Some(err);
                    }
                    // These outcomes are independent of which secret is used.
                    jsonwebtoken::errors::ErrorKind::ExpiredSignature => {
                        return Err(AuthError::TokenExpired)
                    }
                    jsonwebtoken::errors::ErrorKind::InvalidToken => {
                        return Err(AuthError::InvalidToken(format!("Malformed token: {err}")))
                    }
                    _ => return Err(AuthError::InvalidToken(err.to_string())),
                },
            }
        }

        Err(AuthError::InvalidToken(
            last_signature_err
                .map(|e| e.to_string())
                .unwrap_or_else(|| "No verification secret available".to_string()),
        ))
    }

    /// Reject a token whose `token_type` claim is not the one the slot requires
    /// (finding #1). A refresh token presented as a bearer access credential
    /// (or vice-versa) is treated as an invalid token.
    fn ensure_token_type(claims: &Claims, expected: TokenType) -> Result<(), AuthError> {
        if claims.token_type != expected {
            return Err(AuthError::InvalidToken(format!(
                "Wrong token type: expected {expected:?}, got {:?}",
                claims.token_type
            )));
        }
        Ok(())
    }

    /// Verify an **access** token and return its claims.
    ///
    /// Enforces expiry, signature (with rotation-safe backup fallback) and that
    /// the token is an access token — a refresh token presented here is rejected
    /// (finding #1).
    pub async fn verify_token(&self, token: &str) -> Result<Claims, AuthError> {
        let claims = self.decode_with_secrets(token, true).await?;
        Self::ensure_token_type(&claims, TokenType::Access)?;
        Ok(claims)
    }

    /// Verify a **refresh** token and return its claims.
    ///
    /// Like [`Self::verify_token`] but requires the `Refresh` token type; an
    /// access token presented in a refresh slot is rejected (finding #1).
    pub async fn verify_refresh_token(&self, token: &str) -> Result<Claims, AuthError> {
        let claims = self.decode_with_secrets(token, true).await?;
        Self::ensure_token_type(&claims, TokenType::Refresh)?;
        Ok(claims)
    }

    /// Verify the signature and access-token type of a possibly-expired access
    /// token, returning its claims.
    ///
    /// Used only by the refresh endpoint, which must read the subject of an
    /// already-expired access token to bind it to the refresh token (finding
    /// #3). Expiry is intentionally not enforced here; the caller has already
    /// confirmed the access token is expired before reaching this path.
    pub async fn verify_expired_access_claims(&self, token: &str) -> Result<Claims, AuthError> {
        let claims = self.decode_with_secrets(token, false).await?;
        Self::ensure_token_type(&claims, TokenType::Access)?;
        Ok(claims)
    }

    /// Verify a raw JWT token string and return an AuthResponse.
    ///
    /// This is the shared validation path used by both header-based and
    /// query-param authentication. It verifies the JWT signature, checks
    /// the node URL claim (if present and headers are provided), and
    /// confirms the key exists and has not been revoked.
    ///
    /// # Note
    ///
    /// When `headers` is `None`, the `node_url` claim validation is skipped.
    /// All current call sites pass `Some(headers)` — `None` is reserved for
    /// future internal use where no HTTP request headers are available.
    pub async fn verify_token_string(
        &self,
        token: &str,
        headers: Option<&HeaderMap>,
    ) -> Result<AuthResponse, AuthError> {
        if token.is_empty() {
            return Err(AuthError::InvalidRequest(
                "Empty token provided".to_string(),
            ));
        }

        let claims = self.verify_token(token).await?;

        // Check node URL if token has node information
        if let Some(token_node_url) = &claims.node_url {
            if let Some(headers) = headers {
                if let Err(error_msg) = self.validate_node_host(token_node_url, headers) {
                    return Err(AuthError::InvalidToken(error_msg));
                }
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

        // Re-derive effective permissions from the LIVE key rather than trusting
        // the snapshot baked into the token (finding #10). The token's claim acts
        // only as an upper bound: a permission removed from the key after the
        // token was issued must no longer be granted. The result is the
        // intersection of the claimed permissions with the key's current set.
        let live_permissions: std::collections::HashSet<&String> = key.permissions.iter().collect();
        let effective_permissions: Vec<String> = claims
            .permissions
            .into_iter()
            .filter(|perm| live_permissions.contains(perm))
            .collect();

        Ok(AuthResponse {
            is_valid: true,
            key_id: claims.sub,
            permissions: effective_permissions,
        })
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
            .map_err(|e| AuthError::InvalidRequest(format!("Invalid Authorization header: {e}")))?;

        if !auth_header.starts_with("Bearer ") {
            return Err(AuthError::InvalidRequest(
                "Invalid Authorization header format. Expected 'Bearer <token>'".to_string(),
            ));
        }

        let token = auth_header.trim_start_matches("Bearer ").trim();

        self.verify_token_string(token, Some(headers)).await
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

    /// Return the `public_key` field stored for a given `key_id`, if any.
    pub async fn get_public_key_for_key_id(
        &self,
        key_id: &str,
    ) -> Result<Option<String>, AuthError> {
        let key = self
            .key_manager
            .get_key(key_id)
            .await
            .map_err(|e| AuthError::StorageError(e.to_string()))?;
        Ok(key.and_then(|k| k.public_key))
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
        // Verify the refresh token to get the claims. This requires the token to
        // actually be a refresh token (finding #1) — an access token cannot be
        // exchanged for a new pair here.
        let claims = self.verify_refresh_token(refresh_token).await?;

        // Get the key and verify it's valid
        let key = self
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

        // NB: a challenge is a short-lived auth nonce, not a Bearer access
        // token. Its expiry is deliberately NOT mapped to `AuthError::TokenExpired`
        // — that variant signals access-token expiry and drives the SDK's refresh
        // flow, which makes no sense for an expired challenge.
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
    use crate::storage::MemoryStorage;

    fn test_config() -> JwtConfig {
        JwtConfig {
            issuer: "calimero-test".to_string(),
            access_token_expiry: 3600,
            refresh_token_expiry: 30 * 24 * 3600,
        }
    }

    async fn test_manager() -> (TokenManager, Arc<SecretManager>) {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let secret_manager = Arc::new(SecretManager::new(Arc::clone(&storage)));
        secret_manager.initialize().await.unwrap();
        let tm = TokenManager::new(
            test_config(),
            Arc::clone(&storage),
            Arc::clone(&secret_manager),
        );
        (tm, secret_manager)
    }

    #[tokio::test]
    async fn verify_token_succeeds_after_secret_rotation() {
        let (tm, sm) = test_manager().await;
        let (access, _refresh) = tm
            .generate_mock_token_pair("key-1".to_string(), vec!["admin".to_string()], None, None)
            .await
            .unwrap();

        // Token verifies before rotation.
        let claims = tm.verify_token(&access).await.unwrap();
        assert_eq!(claims.sub, "key-1");

        // After a rotation the token was signed with the now-backup secret.
        sm.rotate_secret(SecretType::JwtAuth).await.unwrap();

        // Fix A: verification falls back to the unexpired backup secret.
        let claims = tm
            .verify_token(&access)
            .await
            .expect("token signed with backup secret must still verify");
        assert_eq!(claims.sub, "key-1");
    }

    #[tokio::test]
    async fn verify_token_fails_once_backup_is_evicted() {
        let (tm, sm) = test_manager().await;
        let (access, _refresh) = tm
            .generate_mock_token_pair("key-1".to_string(), vec!["admin".to_string()], None, None)
            .await
            .unwrap();

        // Two rotations push the original signing secret out of the grace window.
        sm.rotate_secret(SecretType::JwtAuth).await.unwrap();
        sm.rotate_secret(SecretType::JwtAuth).await.unwrap();

        let err = tm.verify_token(&access).await.unwrap_err();
        assert!(
            matches!(err, AuthError::InvalidToken(_)),
            "token signed with an evicted secret must be rejected, got {err:?}"
        );
    }

    #[tokio::test]
    async fn verify_token_rejects_unknown_secret() {
        let (tm, _sm) = test_manager().await;

        // A token minted by a completely independent manager (different secret).
        let other_storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let other_sm = Arc::new(SecretManager::new(Arc::clone(&other_storage)));
        other_sm.initialize().await.unwrap();
        let other_tm = TokenManager::new(test_config(), other_storage, other_sm);
        let (foreign, _r) = other_tm
            .generate_mock_token_pair("key-1".to_string(), vec!["admin".to_string()], None, None)
            .await
            .unwrap();

        let err = tm.verify_token(&foreign).await.unwrap_err();
        assert!(
            matches!(err, AuthError::InvalidToken(_)),
            "foreign-signed token must be rejected, got {err:?}"
        );
    }

    // ==========================================================================
    // TOKEN-TYPE ENFORCEMENT (finding #1)
    // ==========================================================================

    #[tokio::test]
    async fn access_token_verifies_as_access() {
        let (tm, _sm) = test_manager().await;
        let (access, _refresh) = tm
            .generate_mock_token_pair("key-1".to_string(), vec!["admin".to_string()], None, None)
            .await
            .unwrap();

        let claims = tm.verify_token(&access).await.unwrap();
        assert_eq!(claims.token_type, TokenType::Access);
    }

    #[tokio::test]
    async fn refresh_token_verifies_as_refresh() {
        let (tm, _sm) = test_manager().await;
        let (_access, refresh) = tm
            .generate_mock_token_pair("key-1".to_string(), vec!["admin".to_string()], None, None)
            .await
            .unwrap();

        let claims = tm.verify_refresh_token(&refresh).await.unwrap();
        assert_eq!(claims.token_type, TokenType::Refresh);
    }

    #[tokio::test]
    async fn refresh_token_rejected_as_access_token() {
        let (tm, _sm) = test_manager().await;
        let (_access, refresh) = tm
            .generate_mock_token_pair("key-1".to_string(), vec!["admin".to_string()], None, None)
            .await
            .unwrap();

        // A refresh token must NOT be usable as a bearer access credential.
        let err = tm.verify_token(&refresh).await.unwrap_err();
        assert!(
            matches!(err, AuthError::InvalidToken(_)),
            "refresh token presented as access must be rejected, got {err:?}"
        );
    }

    #[tokio::test]
    async fn access_token_rejected_in_refresh_slot() {
        let (tm, _sm) = test_manager().await;
        let (access, _refresh) = tm
            .generate_mock_token_pair("key-1".to_string(), vec!["admin".to_string()], None, None)
            .await
            .unwrap();

        // An access token must NOT be exchangeable at the refresh slot.
        let err = tm.verify_refresh_token(&access).await.unwrap_err();
        assert!(
            matches!(err, AuthError::InvalidToken(_)),
            "access token presented at refresh slot must be rejected, got {err:?}"
        );
    }

    #[tokio::test]
    async fn refresh_token_pair_rejects_access_token() {
        let (tm, _sm) = test_manager().await;

        // Store a root key so the underlying key lookup would otherwise succeed.
        let key = crate::storage::models::Key::new_root_key_with_permissions(
            "pk".to_string(),
            "method".to_string(),
            vec!["admin".to_string()],
            None,
        );
        tm.get_key_manager().set_key("key-1", &key).await.unwrap();

        let (access, _refresh) = tm
            .generate_token_pair("key-1".to_string(), vec!["admin".to_string()], None)
            .await
            .unwrap();

        // Refreshing with an access token in the refresh slot must fail.
        let err = tm.refresh_token_pair(&access).await.unwrap_err();
        assert!(
            matches!(err, AuthError::InvalidToken(_)),
            "refresh_token_pair must reject an access token, got {err:?}"
        );
    }

    // ==========================================================================
    // LIVE-KEY PERMISSION RE-DERIVATION (finding #10)
    // ==========================================================================

    #[tokio::test]
    async fn verify_token_string_rederives_perms_from_live_key() {
        let (tm, _sm) = test_manager().await;

        // Key initially holds two permissions.
        let mut key = crate::storage::models::Key::new_root_key_with_permissions(
            "pk".to_string(),
            "method".to_string(),
            vec!["admin".to_string(), "context".to_string()],
            None,
        );
        tm.get_key_manager().set_key("key-1", &key).await.unwrap();

        // Mint a token carrying both permissions.
        let (access, _refresh) = tm
            .generate_token_pair(
                "key-1".to_string(),
                vec!["admin".to_string(), "context".to_string()],
                None,
            )
            .await
            .unwrap();

        // Sanity: before any change, both permissions are present.
        let resp = tm.verify_token_string(&access, None).await.unwrap();
        assert!(resp.permissions.contains(&"admin".to_string()));
        assert!(resp.permissions.contains(&"context".to_string()));

        // Revoke "context" from the LIVE key (keep "admin").
        key.set_permissions(vec!["admin".to_string()]);
        tm.get_key_manager().set_key("key-1", &key).await.unwrap();

        // The still-valid token must no longer grant the revoked permission.
        let resp = tm.verify_token_string(&access, None).await.unwrap();
        assert!(
            resp.permissions.contains(&"admin".to_string()),
            "retained permission must still be granted"
        );
        assert!(
            !resp.permissions.contains(&"context".to_string()),
            "permission removed from the live key must NOT be granted, got {:?}",
            resp.permissions
        );
    }

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
