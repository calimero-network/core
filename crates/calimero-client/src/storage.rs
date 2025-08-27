//! Token storage and management for Calimero client
//! 
//! This module provides the core types and functionality for managing
//! JWT tokens used for API authentication.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

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
        Self {
            access_token,
            refresh_token: None,
            token_type: Some("Bearer".to_string()),
            expires_at: None,
            metadata: HashMap::new(),
        }
    }
    
    /// Create a new JWT token with refresh token
    pub fn with_refresh(access_token: String, refresh_token: String) -> Self {
        Self {
            access_token,
            refresh_token: Some(refresh_token),
            token_type: Some("Bearer".to_string()),
            expires_at: None,
            metadata: HashMap::new(),
        }
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
        self.metadata.insert(key, value);
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
}

impl Default for JwtToken {
    fn default() -> Self {
        Self {
            access_token: String::new(),
            refresh_token: None,
            token_type: Some("Bearer".to_string()),
            expires_at: None,
            metadata: HashMap::new(),
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
        cache.insert(url.to_string(), tokens.clone());
    }
    
    /// Get tokens for a specific URL
    pub async fn get_tokens(&self, url: &str) -> Option<JwtToken> {
        let cache = self.tokens.read().await;
        cache.get(url).cloned()
    }
    
    /// Remove tokens for a specific URL
    pub async fn remove_tokens(&self, url: &str) {
        let mut cache = self.tokens.write().await;
        cache.remove(url);
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
            errors.push("Access token is empty".to_string());
        }
        if is_expired {
            errors.push("Token is expired".to_string());
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
    
    #[test]
    fn test_jwt_token_creation() {
        let token = JwtToken::new("test_token".to_string());
        assert_eq!(token.access_token, "test_token");
        assert_eq!(token.token_type, Some("Bearer".to_string()));
        assert!(!token.is_expired());
    }
    
    #[test]
    fn test_jwt_token_with_refresh() {
        let token = JwtToken::with_refresh("access".to_string(), "refresh".to_string());
        assert_eq!(token.access_token, "access");
        assert_eq!(token.refresh_token, Some("refresh".to_string()));
        assert!(token.has_refresh());
    }
    
    #[test]
    fn test_jwt_token_expiration() {
        let mut token = JwtToken::new("test".to_string());
        assert!(!token.is_expired());
        
        // Set expiration to past
        token.expires_at = Some(chrono::Utc::now().timestamp() - 3600);
        assert!(token.is_expired());
        
        // Set expiration to future
        token.expires_at = Some(chrono::Utc::now().timestamp() + 3600);
        assert!(!token.is_expired());
        assert!(!token.expires_soon(300));
        
        // Set expiration to near future
        token.expires_at = Some(chrono::Utc::now().timestamp() + 150);
        assert!(token.expires_soon(300));
    }
    
    #[test]
    fn test_auth_header() {
        let token = JwtToken::new("test_token".to_string());
        assert_eq!(token.auth_header(), "Bearer test_token");
        
        let mut token = JwtToken::new("test_token".to_string());
        token.token_type = Some("Custom".to_string());
        assert_eq!(token.auth_header(), "Custom test_token");
    }
    
    #[tokio::test]
    async fn test_session_cache() {
        let cache = SessionTokenCache::new();
        let token = JwtToken::new("test".to_string());
        
        // Test storage and retrieval
        cache.store_tokens("http://test.com", &token).await;
        assert!(cache.has_tokens("http://test.com").await);
        
        let retrieved = cache.get_tokens("http://test.com").await;
        assert_eq!(retrieved, Some(token));
        
        // Test removal
        cache.remove_tokens("http://test.com").await;
        assert!(!cache.has_tokens("http://test.com").await);
    }
    
    #[test]
    fn test_token_validation() {
        let token = JwtToken::new("test".to_string());
        let validation = TokenValidation::new(&token);
        
        assert!(validation.is_valid);
        assert!(!validation.is_expired);
        assert!(!validation.expires_soon);
        
        // Test expired token
        let mut expired_token = JwtToken::new("test".to_string());
        expired_token.expires_at = Some(chrono::Utc::now().timestamp() - 3600);
        let expired_validation = TokenValidation::new(&expired_token);
        
        assert!(!expired_validation.is_valid);
        assert!(expired_validation.is_expired);
        assert!(expired_validation.needs_refresh());
    }
}
