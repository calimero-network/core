//! Sync configuration

use std::time::Duration;

/// Configuration for sync orchestration
#[derive(Clone, Debug)]
pub struct SyncConfig {
    /// Timeout for individual sync operations
    pub timeout: Duration,

    /// Maximum number of concurrent syncs
    pub max_concurrent_syncs: usize,

    /// Retry configuration
    pub retry_config: RetryConfig,
}

/// Retry configuration for failed syncs
#[derive(Clone, Debug)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_retries: usize,

    /// Initial backoff duration
    pub initial_backoff: Duration,

    /// Maximum backoff duration
    pub max_backoff: Duration,

    /// Backoff multiplier
    pub backoff_multiplier: f64,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_concurrent_syncs: 10,
            retry_config: RetryConfig::default(),
        }
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
            backoff_multiplier: 2.0,
        }
    }
}

impl SyncConfig {
    /// Create a new sync config with custom timeout
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout,
            ..Default::default()
        }
    }
}
