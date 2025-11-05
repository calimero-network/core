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

    /// Enable periodic sync heartbeat
    pub enable_heartbeat: bool,

    /// Heartbeat interval (periodic sync check)
    pub heartbeat_interval: Duration,
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
            enable_heartbeat: true,
            heartbeat_interval: Duration::from_secs(60),
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

    /// Disable heartbeat (for testing or manual sync)
    pub fn without_heartbeat(mut self) -> Self {
        self.enable_heartbeat = false;
        self
    }
}
