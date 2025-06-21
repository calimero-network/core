use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

/// JWT token pair with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokens {
    pub profile: String,
    pub node_url: Url,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub permissions: Vec<String>,
}

impl AuthTokens {
    pub fn new(
        profile: String,
        node_url: Url,
        access_token: String,
        refresh_token: String,
        expires_at: DateTime<Utc>,
        permissions: Vec<String>,
    ) -> Self {
        Self {
            profile,
            node_url,
            access_token,
            refresh_token,
            expires_at,
            permissions,
        }
    }

    /// Check if the access token is expired
    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at
    }

    /// Check if the token will expire within the given duration
    pub fn expires_within(&self, duration: chrono::Duration) -> bool {
        Utc::now() + duration >= self.expires_at
    }

    /// Get time until expiration
    pub fn time_until_expiry(&self) -> chrono::Duration {
        self.expires_at - Utc::now()
    }
} 