use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use axum::http::HeaderMap;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use {hex, uuid};

use crate::config::JwtConfig;
use crate::secrets::{SecretManager, SecretType};
use crate::storage::models::KeyType;
use crate::storage::{KeyManager, Storage};
use crate::{AuthError, AuthResponse};

/// Storage keyspace prefix for the consumed-refresh-token denylist (finding #2).
///
/// Each successful refresh records the `jti` of the refresh token it just
/// consumed under `system:refresh:consumed:{jti}` with the token's own
/// expiry as the value, so a replay of that exact refresh token can be
/// detected. Entries are only meaningful until the token would have expired
/// anyway (after that the token fails the expiry check regardless), so they
/// are reaped lazily and by a throttled sweep.
const CONSUMED_REFRESH_PREFIX: &str = "system:refresh:consumed:";

/// Storage keyspace prefix mapping a rotated-away client key id to its
/// replacement (`system:refresh:rotated:{old_id}` → `"{new_id} {exp}"`).
///
/// Written on every client-key rotation. When a replayed refresh token names a
/// key id that a later successful rotation already deleted, family revocation
/// chases this chain to find the LIVE key — otherwise revoke-by-sub would
/// silently miss it and the stolen family would stay valid. Entries carry the
/// old refresh token's expiry (after which a replay fails on expiry alone) and
/// are GC'd by the same throttled sweep as the consumed denylist.
const ROTATED_KEY_PREFIX: &str = "system:refresh:rotated:";

/// Upper bound on rotation-chain hops when resolving a family's live key id.
/// Bounds storage reads even if the chain were ever corrupted into a cycle.
const MAX_ROTATION_CHAIN: usize = 32;

/// Minimum seconds between throttled sweeps of the consumed-refresh denylist.
/// The sweep walks the keyspace and drops entries whose recorded expiry has
/// passed, bounding the store's growth without a per-call cost.
const CONSUMED_REFRESH_SWEEP_INTERVAL_SECS: i64 = 3600;

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

/// JWT Token Manager
///
/// This component handles JWT token generation and verification.
#[derive(Clone)]
pub struct TokenManager {
    config: JwtConfig,
    key_manager: KeyManager,
    secret_manager: Arc<SecretManager>,
    /// Backing storage for the consumed-refresh-token denylist (finding #2).
    /// Shares the same backend as keys/secrets.
    storage: Arc<dyn Storage>,
    /// Unix-seconds timestamp of the last consumed-refresh denylist sweep,
    /// shared across clones so the throttle is process-wide.
    last_consumed_sweep: Arc<AtomicI64>,
    /// Serializes the consumed-check + denylist-write of a refresh exchange so
    /// two concurrent requests carrying the same refresh token cannot both pass
    /// the reuse check (TOCTOU). Process-wide (shared across clones) is
    /// sufficient: the storage trait has no compare-and-swap, and the
    /// persistent backend is single-process (RocksDB holds an exclusive file
    /// lock).
    consume_refresh_lock: Arc<tokio::sync::Mutex<()>>,
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
            storage,
            last_consumed_sweep: Arc::new(AtomicI64::new(0)),
            consume_refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
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

        Self::validate_node_host_match(token_node_url, request_host)
    }

    /// Pure host-binding comparison, separated for testability.
    ///
    /// This is only reached for a node-bound token (the caller invokes it
    /// exactly when `node_url` is set). If the request carries no determinable
    /// host, we cannot prove it is addressed to the bound node — fail
    /// **closed**. Letting a missing (or empty) host fall through to `Ok(())`
    /// would let any client strip the `Host`/`X-Forwarded-Host` header and
    /// bypass node binding entirely.
    fn validate_node_host_match(
        token_node_url: &str,
        request_host: Option<&str>,
    ) -> Result<(), String> {
        let request_host = match request_host {
            Some(host) if !host.trim().is_empty() => host,
            // Fail closed: no host on a node-bound token is not trustworthy.
            _ => {
                return Err(
                    "Token is node-bound but the request carries no Host or X-Forwarded-Host \
                     header to validate against"
                        .to_owned(),
                );
            }
        };

        // Skip validation if request is coming from internal auth service.
        // Only allow exact "auth" hostname or "auth:" followed by a numeric port.
        // This prevents bypass attacks using forged headers like "auth:malicious.com".
        if Self::is_internal_auth_service(request_host) {
            return Ok(());
        }

        // Extract the host from the token's node URL. `node_url` is not always a
        // URL (it can carry a client name), so an unparseable value is left to
        // fall through rather than rejected here — the missing-request-host case
        // above is the binding-bypass this guards.
        let token_host = Url::parse(token_node_url)
            .ok()
            .and_then(|url| url.host_str().map(|h| h.to_ascii_lowercase()));

        if let Some(token_host) = token_host {
            // Compare the hosts (handle both with and without port); host
            // names are case-insensitive, so compare lowercased.
            let request_host_without_port = request_host.split(':').next().unwrap_or(request_host);
            let request_host_lc = request_host.to_ascii_lowercase();
            let request_host_without_port_lc = request_host_without_port.to_ascii_lowercase();
            if request_host_without_port_lc != token_host && request_host_lc != token_host {
                return Err(format!(
                    "Token is not valid for this host. Token is for '{token_host}' but request is to '{request_host}'"
                ));
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
            .map_err(|e| AuthError::TokenGenerationFailed(e.into()))?;

        let header = Header::new(Algorithm::HS256);
        encode(
            &header,
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .map_err(|e| AuthError::TokenGenerationFailed(e.into()))
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
            .map_err(|e| AuthError::StorageError(e.into()))?
            .ok_or_else(|| AuthError::InvalidToken("Key not found".to_string()))?;

        if !key.is_valid() {
            return Err(AuthError::TokenRevoked);
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
            .map_err(|e| AuthError::TokenGenerationFailed(e.into()))?;

        if secrets.is_empty() {
            // Every deployment must have at least a primary secret; reaching
            // this point means the secret manager is misconfigured or its
            // storage is unreadable. Log loudly — otherwise every token verify
            // fails with a generic "invalid token" and the root cause is
            // invisible in production.
            tracing::error!("No JWT verification secrets available; rejecting all tokens");
            return Err(AuthError::InvalidToken(
                "No verification secret available".to_string(),
            ));
        }

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
                .expect("loop over non-empty secrets always records a signature error")
                .to_string(),
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

    /// Decode a signature-valid **access** token, skipping expiry enforcement,
    /// and return its claims.
    ///
    /// **This does NOT assert that the token is expired** — a still-valid access
    /// token also passes. It only guarantees signature validity (with the
    /// rotation-safe backup fallback) and the `Access` token type. The refresh
    /// endpoint uses it to read the subject of an access token regardless of
    /// expiry, then applies its own "must be expired" policy on the returned
    /// claims. Any new caller must add its own expiry policy too.
    pub async fn decode_access_claims_ignore_expiry(
        &self,
        token: &str,
    ) -> Result<Claims, AuthError> {
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
            .map_err(|e| AuthError::StorageError(e.into()))?
            .ok_or_else(|| AuthError::InvalidToken("Key not found".to_string()))?;

        if !key.is_valid() {
            return Err(AuthError::TokenRevoked);
        }

        // Re-derive effective permissions from the LIVE key rather than trusting
        // the snapshot baked into the token (finding #10). The token's claim acts
        // only as an upper bound: a permission removed from the key after the
        // token was issued must no longer be granted. The result is the
        // intersection of the claimed permissions with the key's current set.
        // Keys hold a handful of permissions, so a linear scan beats building a
        // set on every verification.
        let effective_permissions: Vec<String> = claims
            .permissions
            .into_iter()
            .filter(|perm| key.permissions.contains(perm))
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
            .map_err(|e| AuthError::StorageError(e.into()))?
            .ok_or_else(|| AuthError::InvalidToken("Key not found".to_string()))?;

        // Revoke the key
        key.revoke();

        // Save the updated key
        self.key_manager
            .set_key(key_id, &key)
            .await
            .map_err(|e| AuthError::StorageError(format!("Failed to update key: {e}").into()))?;

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
            .map_err(|e| AuthError::StorageError(e.into()))?;
        Ok(key.and_then(|k| k.public_key))
    }

    /// Storage key for a consumed-refresh-token denylist entry.
    fn consumed_refresh_key(jti: &str) -> String {
        format!("{CONSUMED_REFRESH_PREFIX}{jti}")
    }

    /// Whether this refresh-token `jti` has already been exchanged (replay guard).
    async fn is_refresh_consumed(&self, jti: &str) -> Result<bool, AuthError> {
        self.storage
            .exists(&Self::consumed_refresh_key(jti))
            .await
            .map_err(|e| AuthError::StorageError(e.into()))
    }

    /// Record a just-consumed refresh-token `jti` so a later replay is detected.
    /// The stored value is the token's own expiry (unix secs); after that the
    /// token fails the expiry check regardless, so the entry is only kept until
    /// then and is reaped by the throttled sweep.
    async fn record_consumed_refresh(&self, jti: &str, exp: u64) -> Result<(), AuthError> {
        self.storage
            .set(&Self::consumed_refresh_key(jti), exp.to_string().as_bytes())
            .await
            .map_err(|e| AuthError::StorageError(e.into()))
    }

    /// Storage key for a rotated-client-key mapping entry.
    fn rotated_key_key(old_id: &str) -> String {
        format!("{ROTATED_KEY_PREFIX}{old_id}")
    }

    /// Record that `old_id` was rotated to `new_id`. `exp` is the expiry of the
    /// refresh token that drove the rotation: past it, a replay of the old
    /// token fails on expiry alone, so the mapping becomes dead weight and is
    /// reaped by the sweep.
    async fn record_rotated_key(
        &self,
        old_id: &str,
        new_id: &str,
        exp: u64,
    ) -> Result<(), AuthError> {
        self.storage
            .set(
                &Self::rotated_key_key(old_id),
                format!("{new_id} {exp}").as_bytes(),
            )
            .await
            .map_err(|e| AuthError::StorageError(e.into()))
    }

    /// Follow the rotation chain from `key_id` to the id that currently exists
    /// in the key store, if any.
    async fn resolve_live_key_id(&self, key_id: &str) -> Result<Option<String>, AuthError> {
        let mut id = key_id.to_string();
        for _ in 0..MAX_ROTATION_CHAIN {
            match self.key_manager.get_key(&id).await {
                Ok(Some(_)) => return Ok(Some(id)),
                Ok(None) => {}
                Err(e) => return Err(AuthError::StorageError(e.into())),
            }
            let next = self
                .storage
                .get(&Self::rotated_key_key(&id))
                .await
                .map_err(|e| AuthError::StorageError(e.into()))?;
            match next
                .as_deref()
                .and_then(|b| std::str::from_utf8(b).ok())
                .and_then(|s| s.split_whitespace().next())
            {
                Some(next_id) => id = next_id.to_string(),
                None => return Ok(None),
            }
        }
        Ok(None)
    }

    /// Revoke the LIVE key of the token family rooted at `key_id` (finding #2).
    ///
    /// A replayed refresh token names the key id it was minted for; for client
    /// keys that id may have been deleted by a later successful rotation, so
    /// revoking by `sub` alone would silently miss the live key and leave the
    /// (presumed stolen) family valid. Chase the rotation mapping first.
    ///
    /// **Root keys are never revoked here.** A user_password login mints its
    /// pair against the ROOT key, so revoking on reuse would revoke the node's
    /// root key — and `verify_credentials` rejects an invalid key while
    /// `list_keys` still reports it, so the bootstrap path can't re-fire either:
    /// the node becomes permanently unauthenticatable. That is a far worse
    /// outcome than the replay itself, which is already refused by the
    /// single-use denylist. (The HTTP layer applies the same rule — see
    /// `delete_key_handler`, which refuses to revoke the last active root key.)
    /// Revoking a root key stays an explicit, authenticated admin action.
    async fn revoke_token_family(&self, key_id: &str) -> Result<(), AuthError> {
        let Some(live_id) = self.resolve_live_key_id(key_id).await? else {
            return Err(AuthError::InvalidToken(format!(
                "No live key found for token family {key_id}"
            )));
        };

        let key = self
            .key_manager
            .get_key(&live_id)
            .await
            .map_err(|e| AuthError::StorageError(e.into()))?
            .ok_or_else(|| AuthError::InvalidToken("Key not found".to_string()))?;

        if key.key_type == KeyType::Root {
            tracing::warn!(
                "Refresh-token reuse on ROOT key {live_id}: rejecting the replay but NOT \
                 revoking the key (revoking it would lock the node out permanently). \
                 Re-authenticate; revoke the key explicitly if compromise is suspected."
            );
            return Ok(());
        }

        self.revoke_client_tokens(&live_id).await
    }

    /// Throttled GC of expired consumed-refresh entries. Runs at most once per
    /// [`CONSUMED_REFRESH_SWEEP_INTERVAL_SECS`] process-wide (the timestamp is
    /// shared across `TokenManager` clones), bounding the denylist's growth
    /// without a per-refresh cost. Best-effort: failures are logged, not fatal.
    async fn maybe_sweep_consumed_refresh(&self) {
        let now = Utc::now().timestamp();
        let last = self.last_consumed_sweep.load(Ordering::Relaxed);
        if now - last < CONSUMED_REFRESH_SWEEP_INTERVAL_SECS {
            return;
        }
        // Claim the sweep slot; if another clone won the race, let it run.
        if self
            .last_consumed_sweep
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let keys = match self.storage.list_keys(CONSUMED_REFRESH_PREFIX).await {
            Ok(keys) => keys,
            Err(e) => {
                tracing::warn!("consumed-refresh sweep: list_keys failed: {e}");
                return;
            }
        };
        for key in keys {
            if let Ok(Some(bytes)) = self.storage.get(&key).await {
                let expired = std::str::from_utf8(&bytes)
                    .ok()
                    .and_then(|s| s.parse::<i64>().ok())
                    .is_some_and(|exp| exp <= now);
                if expired {
                    let _ = self.storage.delete(&key).await;
                }
            }
        }

        // Same GC for rotated-key mappings (value = "<new_id> <exp>").
        let keys = match self.storage.list_keys(ROTATED_KEY_PREFIX).await {
            Ok(keys) => keys,
            Err(e) => {
                tracing::warn!("rotated-key sweep: list_keys failed: {e}");
                return;
            }
        };
        for key in keys {
            if let Ok(Some(bytes)) = self.storage.get(&key).await {
                let expired = std::str::from_utf8(&bytes)
                    .ok()
                    .and_then(|s| s.split_whitespace().nth(1))
                    .and_then(|s| s.parse::<i64>().ok())
                    .is_some_and(|exp| exp <= now);
                if expired {
                    let _ = self.storage.delete(&key).await;
                }
            }
        }
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

        // Look up the key, but do NOT bail on a missing one yet: for a client
        // key, a replayed (already-consumed) refresh token names a key id that
        // a later successful rotation deleted — reuse detection below must still
        // fire for it and revoke the LIVE key via the rotation chain.
        let key = self.key_manager.get_key(&claims.sub).await.map_err(|e| {
            tracing::error!("Storage error while getting key {}: {}", claims.sub, e);
            AuthError::StorageError(e.into())
        })?;

        // Single-use enforcement (finding #2): atomically claim this refresh
        // token's jti BEFORE minting anything. The lock makes the consumed-check
        // and the denylist write one critical section, so two concurrent
        // requests carrying the same refresh token cannot both pass the check.
        // Recording before minting also fixes the failure-mode asymmetry: if the
        // denylist write fails, the exchange aborts with nothing issued (the
        // client can safely retry with the same token); a mint failure after the
        // write burns the refresh token instead — fail closed, the client
        // re-authenticates — rather than ever leaving a minted-but-unrecorded
        // pair whose refresh token is still exchangeable.
        {
            let _consume_guard = self.consume_refresh_lock.lock().await;
            if self.is_refresh_consumed(&claims.jti).await? {
                tracing::warn!(
                    "Refresh token reuse detected for subject {} (jti {}); revoking family",
                    claims.sub,
                    claims.jti
                );
                // Best-effort family revocation; reject regardless of its outcome.
                if let Err(e) = self.revoke_token_family(&claims.sub).await {
                    tracing::error!("Failed to revoke token family for {}: {}", claims.sub, e);
                }
                return Err(AuthError::TokenReuse);
            }

            // Not a replay: from here on the key must exist and be valid.
            match &key {
                None => {
                    tracing::error!("Key not found: {}", claims.sub);
                    return Err(AuthError::InvalidToken(format!(
                        "Key not found: {}",
                        claims.sub
                    )));
                }
                Some(key) if !key.is_valid() => {
                    return Err(AuthError::InvalidToken("Key is not valid".to_string()));
                }
                Some(_) => {}
            }

            self.record_consumed_refresh(&claims.jti, claims.exp)
                .await?;
        }

        let key = key.expect("checked Some and valid under the consume lock");

        // GC the denylist and rotation mappings off the refresh hot path; the
        // sweep is throttled internally and best-effort.
        let sweeper = self.clone();
        drop(tokio::spawn(async move {
            sweeper.maybe_sweep_consumed_refresh().await;
        }));

        // Captured before the match moves `claims.sub`/`claims.permissions`.
        let rotated_from_exp = claims.exp;

        let result = match key.key_type {
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

                // Log only short id prefixes; the full client key ids are
                // sensitive and add noise to debug logs.
                tracing::debug!(
                    from = claims.sub.get(..8).unwrap_or(claims.sub.as_str()),
                    to = new_client_id.get(..8).unwrap_or(new_client_id.as_str()),
                    "Rotating client key (ids truncated)"
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
                    return Err(AuthError::StorageError(
                        format!("Failed to store new client key during rotation: {e}").into(),
                    ));
                }

                tracing::debug!("Successfully stored new client key: {}", new_client_id);

                // Record old -> new id BEFORE deleting the old key, so a later
                // replay of the old refresh token can chase the chain and revoke
                // the live key (see revoke_token_family). Best-effort: a missing
                // mapping degrades reuse handling, it doesn't break the refresh.
                if let Err(e) = self
                    .record_rotated_key(&claims.sub, &new_client_id, rotated_from_exp)
                    .await
                {
                    tracing::warn!(
                        "Failed to record key-rotation mapping {} -> {}: {}",
                        claims.sub,
                        new_client_id,
                        e
                    );
                }

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
        };

        result
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

    // ==========================================================================
    // REFRESH ROTATION + REUSE DETECTION (finding #2)
    // ==========================================================================

    #[tokio::test]
    async fn refresh_rotates_and_consumed_token_is_reuse_rejected() {
        let (tm, _sm) = test_manager().await;
        let key = crate::storage::models::Key::new_root_key_with_permissions(
            "pk".to_string(),
            "method".to_string(),
            vec!["admin".to_string()],
            None,
        );
        tm.get_key_manager().set_key("key-1", &key).await.unwrap();

        let (_access, refresh) = tm
            .generate_token_pair("key-1".to_string(), vec!["admin".to_string()], None)
            .await
            .unwrap();

        // First exchange succeeds and the refresh token rotates.
        let (_a2, refresh2) = tm.refresh_token_pair(&refresh).await.unwrap();
        assert_ne!(refresh, refresh2, "refresh token must rotate on exchange");

        // Replaying the original (now consumed) refresh token is detected as reuse.
        let err = tm.refresh_token_pair(&refresh).await.unwrap_err();
        assert!(
            matches!(err, AuthError::TokenReuse),
            "replayed refresh token must be rejected as reuse, got {err:?}"
        );

        // This family is rooted at a ROOT key, so the replay is refused but the
        // key is NOT revoked — revoking it would brick the node (see
        // `root_key_survives_refresh_reuse`). The rotated token keeps working.
        assert!(
            tm.refresh_token_pair(&refresh2).await.is_ok(),
            "a root key's rotated refresh token must survive someone else's replay"
        );
    }

    #[tokio::test]
    async fn fresh_refresh_token_is_not_flagged_as_reuse() {
        let (tm, _sm) = test_manager().await;
        let key = crate::storage::models::Key::new_root_key_with_permissions(
            "pk".to_string(),
            "method".to_string(),
            vec!["admin".to_string()],
            None,
        );
        tm.get_key_manager().set_key("key-1", &key).await.unwrap();

        let (_access, refresh) = tm
            .generate_token_pair("key-1".to_string(), vec!["admin".to_string()], None)
            .await
            .unwrap();

        // A never-before-exchanged refresh token must succeed exactly once.
        assert!(
            tm.refresh_token_pair(&refresh).await.is_ok(),
            "a fresh refresh token must be accepted on first use"
        );
    }

    #[tokio::test]
    async fn concurrent_refresh_of_same_token_yields_exactly_one_success() {
        // TOCTOU regression (review finding): the consumed-check and the
        // denylist write are one critical section, so two racing requests with
        // the same refresh token must not both mint a pair.
        let (tm, _sm) = test_manager().await;
        let key = crate::storage::models::Key::new_root_key_with_permissions(
            "pk".to_string(),
            "method".to_string(),
            vec!["admin".to_string()],
            None,
        );
        tm.get_key_manager().set_key("key-1", &key).await.unwrap();

        let (_access, refresh) = tm
            .generate_token_pair("key-1".to_string(), vec!["admin".to_string()], None)
            .await
            .unwrap();

        let tm2 = tm.clone();
        let (r1, r2) = tokio::join!(
            tm.refresh_token_pair(&refresh),
            tm2.refresh_token_pair(&refresh)
        );
        let successes = [&r1, &r2].iter().filter(|r| r.is_ok()).count();
        assert_eq!(
            successes,
            1,
            "exactly one of two concurrent exchanges of the same refresh token \
             may succeed, got r1={:?} r2={:?}",
            r1.is_ok(),
            r2.is_ok()
        );
    }

    #[tokio::test]
    async fn root_key_survives_refresh_reuse() {
        // Node-brick regression: a user_password login mints its pair against the
        // ROOT key, so revoking the family on reuse would revoke the root key —
        // and then verify_credentials rejects it while list_keys still reports it,
        // so bootstrap can't re-fire: the node can never be authenticated again.
        // Two tabs sharing one token bundle are enough to trigger this, so the
        // replay must be REJECTED without revoking the key.
        let (tm, _sm) = test_manager().await;
        let key = crate::storage::models::Key::new_root_key_with_permissions(
            "pk".to_string(),
            "user_password".to_string(),
            vec!["admin".to_string()],
            None,
        );
        tm.get_key_manager().set_key("root-1", &key).await.unwrap();

        let (_access, refresh) = tm
            .generate_token_pair("root-1".to_string(), vec!["admin".to_string()], None)
            .await
            .unwrap();

        let (_a2, refresh2) = tm.refresh_token_pair(&refresh).await.unwrap();

        // Replaying the consumed refresh token is still refused...
        let err = tm.refresh_token_pair(&refresh).await.unwrap_err();
        assert!(
            matches!(err, AuthError::TokenReuse),
            "replayed root refresh token must be rejected as reuse, got {err:?}"
        );

        // ...but the root key MUST remain valid, or the node is bricked.
        let root = tm
            .get_key_manager()
            .get_key("root-1")
            .await
            .unwrap()
            .expect("root key must still exist");
        assert!(
            root.is_valid(),
            "root key must NOT be revoked by refresh-token reuse — that would \
             permanently lock the node out"
        );

        // And the legitimately rotated token still works, so the user is not
        // logged out by someone else's replay.
        assert!(
            tm.refresh_token_pair(&refresh2).await.is_ok(),
            "the live rotated refresh token must keep working"
        );
    }

    #[tokio::test]
    async fn replayed_client_refresh_after_rotation_revokes_live_key() {
        // Review finding: after a client-key rotation deletes the old key id,
        // revoking the family by the replayed token's `sub` finds no key and the
        // live (rotated) key silently survives. The rotation mapping must let
        // reuse handling chase and revoke the LIVE key.
        let (tm, _sm) = test_manager().await;
        let root = crate::storage::models::Key::new_root_key_with_permissions(
            "pk".to_string(),
            "method".to_string(),
            vec!["admin".to_string()],
            None,
        );
        tm.get_key_manager().set_key("root-1", &root).await.unwrap();
        let key = crate::storage::models::Key::new_client_key(
            "root-1".to_string(),
            "client".to_string(),
            vec!["context".to_string()],
            None,
        );
        tm.get_key_manager()
            .set_key("client-1", &key)
            .await
            .unwrap();

        let (_access, refresh) = tm
            .generate_token_pair("client-1".to_string(), vec!["context".to_string()], None)
            .await
            .unwrap();

        // First exchange rotates the client key id and deletes "client-1".
        let (_a2, refresh2) = tm.refresh_token_pair(&refresh).await.unwrap();
        assert!(
            tm.get_key_manager()
                .get_key("client-1")
                .await
                .unwrap()
                .is_none(),
            "old client key id must be deleted by rotation"
        );

        // Replaying the ORIGINAL refresh token is reuse; the family's LIVE
        // (rotated) key must be revoked even though "client-1" is gone.
        let err = tm.refresh_token_pair(&refresh).await.unwrap_err();
        assert!(
            matches!(err, AuthError::TokenReuse),
            "replayed client refresh token must be rejected as reuse, got {err:?}"
        );

        // The rotated refresh token no longer works: its key was revoked.
        let err2 = tm.refresh_token_pair(&refresh2).await.unwrap_err();
        assert!(
            matches!(err2, AuthError::InvalidToken(_)),
            "rotated token must fail after family revocation, got {err2:?}"
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

    // --- #11: node-host binding fail-closed -----------------------------

    #[test]
    fn test_validate_node_host_match_exact_accepted() {
        assert!(TokenManager::validate_node_host_match(
            "https://node.example.com",
            Some("node.example.com"),
        )
        .is_ok());
        assert!(TokenManager::validate_node_host_match(
            "https://node.example.com",
            Some("node.example.com:8443"),
        )
        .is_ok());
    }

    #[test]
    fn test_validate_node_host_match_suffix_attack_rejected() {
        assert!(TokenManager::validate_node_host_match(
            "https://node.example.com",
            Some("node.example.com.attacker.com"),
        )
        .is_err());
    }

    #[test]
    fn test_validate_node_host_match_absent_host_fails_closed() {
        // No host present at all -> reject (previously this passed).
        assert!(TokenManager::validate_node_host_match("https://node.example.com", None).is_err());
        // Empty / whitespace host header -> reject.
        assert!(
            TokenManager::validate_node_host_match("https://node.example.com", Some("")).is_err()
        );
        assert!(
            TokenManager::validate_node_host_match("https://node.example.com", Some("   "))
                .is_err()
        );
    }

    #[test]
    fn test_validate_node_host_match_internal_auth_service_allowed() {
        assert!(TokenManager::validate_node_host_match(
            "https://node.example.com",
            Some("auth:3001")
        )
        .is_ok());
    }

    async fn test_token_manager() -> TokenManager {
        use crate::config::JwtConfig;
        use crate::secrets::SecretManager;
        use crate::storage::providers::memory::MemoryStorage;
        use crate::storage::Storage;

        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let secret_manager = Arc::new(SecretManager::new(Arc::clone(&storage)));
        secret_manager.initialize().await.unwrap();
        TokenManager::new(
            JwtConfig {
                issuer: "test".to_string(),
                access_token_expiry: 3600,
                refresh_token_expiry: 86400,
            },
            storage,
            secret_manager,
        )
    }

    fn headers_with(name: &'static str, value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        // `insert` returns the previous value (an `Option`), not a `Result`;
        // there is no error to handle here.
        drop(h.insert(name, value.parse().unwrap()));
        h
    }

    /// A spoofed `X-Forwarded-Host` for a different node must NOT validate a
    /// token minted for this node — this is the cross-node host-spoof guard.
    /// `X-Forwarded-Host` is preferred over `Host`, so an attacker forging it
    /// must still be rejected.
    #[tokio::test]
    async fn forwarded_host_spoof_is_rejected_cross_node() {
        let tm = test_token_manager().await;
        let token_node = "http://node-a.example:2428";

        // Matching forwarded host: accepted.
        assert!(tm
            .validate_node_host(
                token_node,
                &headers_with("X-Forwarded-Host", "node-a.example")
            )
            .is_ok());

        // Spoofed forwarded host for a different node: rejected, even though the
        // attacker controls the header.
        assert!(tm
            .validate_node_host(token_node, &headers_with("X-Forwarded-Host", "node-b.evil"))
            .is_err());

        // A forged "auth:<non-numeric>" must not be treated as the internal
        // service bypass (it is not a numeric port), so it is rejected.
        assert!(tm
            .validate_node_host(
                token_node,
                &headers_with("X-Forwarded-Host", "auth:evil.com")
            )
            .is_err());

        // The genuine internal-service host (numeric port) still bypasses.
        assert!(tm
            .validate_node_host(token_node, &headers_with("X-Forwarded-Host", "auth:2428"))
            .is_ok());
    }
}
