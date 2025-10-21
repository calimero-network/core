use core::time::Duration;
use serde::{Deserialize, Serialize};

/// SSE server configuration
#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct SseConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
}

impl SseConfig {
    #[must_use]
    pub const fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}

/// Retry timeout for client reconnection (in milliseconds)
pub const SSE_RETRY_TIMEOUT_MS: u64 = 3000;

/// Session expiry time (24 hours in seconds)
pub const SESSION_EXPIRY_SECS: u64 = 24 * 60 * 60;

/// Scope for SSE sessions in storage
pub const SSE_SESSION_SCOPE: [u8; 16] = *b"sse-sessions\0\0\0\0";

/// Get SSE retry timeout as Duration
#[must_use]
pub fn retry_timeout() -> Duration {
    Duration::from_millis(SSE_RETRY_TIMEOUT_MS)
}
