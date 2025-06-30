use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

/// Simple in-memory cache for external connection tokens
/// These tokens are only kept for the duration of the session
#[derive(Debug, Default)]
pub struct SessionTokenCache {
    tokens: Mutex<HashMap<String, JwtToken>>,
}

impl SessionTokenCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store tokens for an external connection (session only)
    pub fn store_tokens(&self, url: &Url, tokens: &JwtToken) {
        let key = format!("external_{}", url.host_str().unwrap_or("unknown"));
        let mut cache = self.tokens.lock().unwrap();
        drop(cache.insert(key, tokens.clone()));
    }

    /// Get tokens for an external connection
    pub fn get_tokens(&self, url: &Url) -> Option<JwtToken> {
        let key = format!("external_{}", url.host_str().unwrap_or("unknown"));
        let cache = self.tokens.lock().unwrap();
        cache.get(&key).cloned()
    }

    /// Update tokens for an external connection
    pub fn update_tokens(&self, url: &Url, tokens: &JwtToken) {
        self.store_tokens(url, tokens);
    }

    /// Clear all cached tokens
    pub fn clear_all(&self) {
        let mut cache = self.tokens.lock().unwrap();
        cache.clear();
    }
}

/// Global session cache instance
static SESSION_CACHE: OnceLock<Arc<SessionTokenCache>> = OnceLock::new();

/// Get the global session cache instance
pub fn get_session_cache() -> &'static Arc<SessionTokenCache> {
    SESSION_CACHE.get_or_init(|| Arc::new(SessionTokenCache::new()))
}
