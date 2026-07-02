//! Token storage and management for Calimero client
//!
//! This module provides the core types and functionality for managing
//! JWT tokens used for API authentication.

// Standard library
use std::collections::HashMap;
use std::sync::Arc;

// External crates
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use zeroize::Zeroize;

/// Decode the `exp` (expiry) claim from a JWT access token without verifying
/// its signature.
///
/// The client never validates JWT signatures — that is the server's job — but
/// the unsigned `exp` claim is still useful for *proactively* refreshing a token
/// before it lapses, saving a wasted 401 round-trip. A malformed or `exp`-less
/// token simply yields `None`, in which case callers treat the token as
/// "expiry unknown" and fall back to reactive (401-driven) refresh.
fn decode_jwt_exp(access_token: &str) -> Option<i64> {
    // JWT layout: header.payload.signature — all base64url (no padding).
    let payload_b64 = access_token.split('.').nth(1)?;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    claims.get("exp")?.as_i64()
}

/// JWT token pair for API authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtToken {
    /// Access token for API requests
    pub access_token: String,
    /// Refresh token for renewing access tokens
    pub refresh_token: Option<String>,
    /// Token type (usually "Bearer")
    pub token_type: Option<String>,
    /// Expiration timestamp
    pub expires_at: Option<i64>,
    /// Additional token metadata
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl JwtToken {
    /// Create a new JWT token
    pub fn new(access_token: String) -> Self {
        let expires_at = decode_jwt_exp(&access_token);
        Self {
            access_token,
            refresh_token: None,
            token_type: Some("Bearer".to_owned()),
            expires_at,
            metadata: HashMap::new(),
        }
    }

    /// Create a new JWT token with refresh token
    pub fn with_refresh(access_token: String, refresh_token: String) -> Self {
        let expires_at = decode_jwt_exp(&access_token);
        Self {
            access_token,
            refresh_token: Some(refresh_token),
            token_type: Some("Bearer".to_owned()),
            expires_at,
            metadata: HashMap::new(),
        }
    }

    /// Whether this token carries usable credentials.
    ///
    /// An empty access token is treated as "no credentials" rather than a valid
    /// bearer value — a logout that persists `JwtToken::default()` (empty access
    /// token) must not be mistaken for an authenticated session and sent as the
    /// meaningless header `Authorization: Bearer `.
    pub fn is_usable(&self) -> bool {
        !self.access_token.is_empty()
    }

    /// Check if the token is expired
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            let now = chrono::Utc::now().timestamp();
            now >= expires_at
        } else {
            false // No expiration set
        }
    }

    /// Check if the token will expire soon (within the given seconds)
    pub fn expires_soon(&self, within_seconds: i64) -> bool {
        if let Some(expires_at) = self.expires_at {
            let now = chrono::Utc::now().timestamp();
            let threshold = expires_at - within_seconds;
            now >= threshold
        } else {
            false
        }
    }

    /// Get the authorization header value
    pub fn auth_header(&self) -> String {
        let token_type = self.token_type.as_deref().unwrap_or("Bearer");
        format!("{} {}", token_type, self.access_token)
    }

    /// Add metadata to the token
    pub fn with_metadata(mut self, key: String, value: serde_json::Value) -> Self {
        drop(self.metadata.insert(key, value));
        self
    }

    /// Get metadata value
    pub fn get_metadata(&self, key: &str) -> Option<&serde_json::Value> {
        self.metadata.get(key)
    }

    /// Check if the token has a refresh token
    pub fn has_refresh(&self) -> bool {
        self.refresh_token.is_some()
    }

    /// Produce an updated token that adopts `incoming`'s access token while
    /// **preserving** prior fields the incoming token doesn't carry.
    ///
    /// A refresh response typically returns only a new access/refresh token
    /// pair, with no `expires_at`, `token_type`, or metadata. Blindly replacing
    /// the stored record with that response would silently discard the metadata
    /// (e.g. auth-provider info) and any previously-known expiry. Here `incoming`
    /// wins where it has a value; otherwise `self`'s value is retained, and
    /// metadata is unioned (incoming keys override).
    #[must_use]
    pub fn merged_with(&self, incoming: &Self) -> Self {
        let mut metadata = self.metadata.clone();
        metadata.extend(incoming.metadata.clone());

        Self {
            access_token: incoming.access_token.clone(),
            refresh_token: incoming
                .refresh_token
                .clone()
                .or_else(|| self.refresh_token.clone()),
            token_type: incoming
                .token_type
                .clone()
                .or_else(|| self.token_type.clone()),
            expires_at: incoming.expires_at.or(self.expires_at),
            metadata,
        }
    }
}

impl Default for JwtToken {
    fn default() -> Self {
        Self {
            access_token: String::new(),
            refresh_token: None,
            token_type: Some("Bearer".to_owned()),
            expires_at: None,
            metadata: HashMap::new(),
        }
    }
}

impl Drop for JwtToken {
    /// Scrub secret material from memory when a token is dropped so freed heap
    /// pages don't retain bearer/refresh secrets in plaintext.
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(refresh) = self.refresh_token.as_mut() {
            refresh.zeroize();
        }
    }
}

impl PartialEq for JwtToken {
    fn eq(&self, other: &Self) -> bool {
        self.access_token == other.access_token
    }
}

impl Eq for JwtToken {}

impl std::hash::Hash for JwtToken {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.access_token.hash(state);
    }
}

/// In-memory token cache for session management
#[derive(Debug, Clone)]
pub struct SessionTokenCache {
    tokens: Arc<RwLock<HashMap<String, JwtToken>>>,
}

impl SessionTokenCache {
    /// Create a new session token cache
    pub fn new() -> Self {
        Self {
            tokens: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Store tokens for a specific URL
    pub async fn store_tokens(&self, url: &str, tokens: &JwtToken) {
        let mut cache = self.tokens.write().await;
        drop(cache.insert(url.to_owned(), tokens.clone()));
    }

    /// Get tokens for a specific URL
    pub async fn get_tokens(&self, url: &str) -> Option<JwtToken> {
        let cache = self.tokens.read().await;
        cache.get(url).cloned()
    }

    /// Remove tokens for a specific URL
    pub async fn remove_tokens(&self, url: &str) {
        let mut cache = self.tokens.write().await;
        drop(cache.remove(url));
    }

    /// Clear all cached tokens
    pub async fn clear_all(&self) {
        let mut cache = self.tokens.write().await;
        cache.clear();
    }

    /// Check if tokens exist for a URL
    pub async fn has_tokens(&self, url: &str) -> bool {
        let cache = self.tokens.read().await;
        cache.contains_key(url)
    }

    /// Get all cached URLs
    pub async fn get_cached_urls(&self) -> Vec<String> {
        let cache = self.tokens.read().await;
        cache.keys().cloned().collect()
    }
}

impl Default for SessionTokenCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Global session cache instance
static SESSION_CACHE: once_cell::sync::Lazy<SessionTokenCache> =
    once_cell::sync::Lazy::new(SessionTokenCache::new);

/// Get the global session cache instance
pub fn get_session_cache() -> SessionTokenCache {
    SESSION_CACHE.clone()
}

/// Token validation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenValidation {
    /// Whether the token is valid
    pub is_valid: bool,
    /// Whether the token is expired
    pub is_expired: bool,
    /// Whether the token will expire soon
    pub expires_soon: bool,
    /// Time until expiration in seconds (negative if expired)
    pub expires_in: i64,
    /// Validation errors if any
    pub errors: Vec<String>,
}

impl TokenValidation {
    /// Create a validation result for a token
    pub fn new(token: &JwtToken) -> Self {
        let now = chrono::Utc::now().timestamp();
        let expires_in = token.expires_at.unwrap_or(0) - now;
        let is_expired = expires_in <= 0;
        let expires_soon = expires_in > 0 && expires_in <= 300; // 5 minutes

        let mut errors = Vec::new();
        if token.access_token.is_empty() {
            errors.push("Access token is empty".to_owned());
        }
        if is_expired {
            errors.push("Token is expired".to_owned());
        }

        Self {
            is_valid: errors.is_empty() && !is_expired,
            is_expired,
            expires_soon,
            expires_in,
            errors,
        }
    }

    /// Check if the token needs refresh
    pub fn needs_refresh(&self) -> bool {
        self.expires_soon || self.is_expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt_with_exp(exp: i64) -> String {
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("{{\"exp\":{exp}}}"));
        // header and signature segments are irrelevant to `exp` extraction.
        format!("aGVhZGVy.{payload}.c2ln")
    }

    #[test]
    fn decode_exp_from_valid_jwt() {
        assert_eq!(
            decode_jwt_exp(&jwt_with_exp(9_999_999_999)),
            Some(9_999_999_999)
        );
    }

    #[test]
    fn decode_exp_handles_non_jwt_and_missing_claim() {
        // Opaque (non-JWT) token → no expiry.
        assert_eq!(decode_jwt_exp("opaque-token"), None);
        // Well-formed JWT without an `exp` claim → no expiry.
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"sub":"x"}"#);
        assert_eq!(decode_jwt_exp(&format!("h.{payload}.s")), None);
    }

    #[test]
    fn constructors_populate_expiry_from_jwt() {
        let token = jwt_with_exp(9_999_999_999);
        assert_eq!(JwtToken::new(token.clone()).expires_at, Some(9_999_999_999));
        assert_eq!(
            JwtToken::with_refresh(token, "r".to_owned()).expires_at,
            Some(9_999_999_999)
        );
    }

    #[test]
    fn expired_and_soon_reflect_exp() {
        // Far in the past → expired; far future → not expired, not soon.
        assert!(JwtToken::new(jwt_with_exp(1)).is_expired());
        let fresh = JwtToken::new(jwt_with_exp(9_999_999_999));
        assert!(!fresh.is_expired());
        assert!(!fresh.expires_soon(30));
    }

    #[test]
    fn empty_access_token_is_not_usable() {
        assert!(!JwtToken::default().is_usable());
        assert!(JwtToken::new("something".to_owned()).is_usable());
    }

    #[test]
    fn merged_with_preserves_prior_metadata_and_expiry() {
        let mut existing = JwtToken::new("old-access".to_owned())
            .with_metadata("auth_type".to_owned(), serde_json::json!("oauth"));
        existing.expires_at = Some(1_234);

        // Incoming refresh carries only a new access/refresh pair (no exp, no metadata).
        let incoming = JwtToken {
            access_token: "new-access".to_owned(),
            refresh_token: Some("new-refresh".to_owned()),
            token_type: None,
            expires_at: None,
            metadata: HashMap::new(),
        };

        let merged = existing.merged_with(&incoming);
        assert_eq!(merged.access_token, "new-access");
        assert_eq!(merged.refresh_token.as_deref(), Some("new-refresh"));
        // Prior expiry and metadata survive the update.
        assert_eq!(merged.expires_at, Some(1_234));
        assert_eq!(
            merged.get_metadata("auth_type"),
            Some(&serde_json::json!("oauth"))
        );
    }
}
