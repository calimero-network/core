use std::num::NonZeroUsize;

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use tracing::debug;

/// Performance optimization settings
pub struct PerformanceConfig {
    /// Maximum artifact size for lightweight processing (bytes)
    pub lightweight_threshold: usize,
    /// Whether to enable lightweight delta processing
    pub enable_lightweight_processing: bool,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            lightweight_threshold: 1024, // 1KB
            enable_lightweight_processing: true,
        }
    }
}

/// Performance optimization service
pub struct PerformanceService {
    config: PerformanceConfig,
}

impl PerformanceService {
    pub fn new(config: PerformanceConfig) -> Self {
        Self { config }
    }

    /// Check if a delta should use lightweight processing
    pub fn should_use_lightweight_processing(
        &self,
        artifact_size: usize,
        is_state_op: bool,
    ) -> bool {
        if !self.config.enable_lightweight_processing {
            return false;
        }

        // Skip WASM execution for small updates that aren't state operations
        artifact_size < self.config.lightweight_threshold && !is_state_op
    }

    /// Apply lightweight delta processing
    pub fn apply_lightweight_delta(
        &self,
        context_id: &ContextId,
        executor: &PublicKey,
        artifact_size: usize,
    ) {
        debug!(
            context_id=%context_id,
            executor=%executor,
            artifact_size,
            "Applying lightweight delta processing"
        );
    }
}
