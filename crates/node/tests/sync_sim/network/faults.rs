//! Fault injection configuration.
//!
//! See spec §13 - Fault Injection.

use std::ops::Range;

/// Network and node fault configuration.
#[derive(Debug, Clone)]
pub struct FaultConfig {
    // =========================================================================
    // Network Faults
    // =========================================================================
    /// Base network latency in milliseconds.
    pub base_latency_ms: u64,

    /// Latency jitter in milliseconds (±).
    pub latency_jitter_ms: u64,

    /// Message loss probability [0.0, 1.0].
    pub message_loss_rate: f64,

    /// Reorder window in milliseconds.
    /// Messages within this window may be delivered out of order.
    pub reorder_window_ms: u64,

    /// Message duplication probability [0.0, 1.0].
    pub duplicate_rate: f64,

    // =========================================================================
    // Partition Faults
    // =========================================================================
    /// Probability of spontaneous partition per tick.
    pub partition_probability: f64,

    /// Duration range for partitions in milliseconds.
    pub partition_duration_ms: Range<u64>,

    // =========================================================================
    // Node Faults
    // =========================================================================
    /// Probability of node crash per tick.
    pub crash_probability: f64,

    /// Delay range before restart in milliseconds.
    pub restart_delay_ms: Range<u64>,

    /// Slow node processing delay multiplier.
    pub slow_node_factor: f64,

    // =========================================================================
    // Concurrent Operations
    // =========================================================================
    /// Probability of write during sync.
    pub write_during_sync_rate: f64,
}

impl Default for FaultConfig {
    fn default() -> Self {
        Self {
            // Reasonable defaults for basic testing
            base_latency_ms: 10,
            latency_jitter_ms: 5,
            message_loss_rate: 0.0,
            reorder_window_ms: 0,
            duplicate_rate: 0.0,
            partition_probability: 0.0,
            partition_duration_ms: 0..0,
            crash_probability: 0.0,
            restart_delay_ms: 0..0,
            slow_node_factor: 1.0,
            write_during_sync_rate: 0.0,
        }
    }
}

impl FaultConfig {
    /// Create a config with no faults (instant delivery, no loss).
    pub fn none() -> Self {
        Self {
            base_latency_ms: 0,
            latency_jitter_ms: 0,
            ..Default::default()
        }
    }

    /// Create a config for light chaos testing.
    pub fn light_chaos() -> Self {
        Self {
            base_latency_ms: 10,
            latency_jitter_ms: 5,
            message_loss_rate: 0.01,
            reorder_window_ms: 20,
            duplicate_rate: 0.01,
            ..Default::default()
        }
    }

    /// Create a config for heavy chaos testing.
    pub fn heavy_chaos() -> Self {
        Self {
            base_latency_ms: 50,
            latency_jitter_ms: 25,
            message_loss_rate: 0.1,
            reorder_window_ms: 100,
            duplicate_rate: 0.05,
            partition_probability: 0.01,
            partition_duration_ms: 100..500,
            crash_probability: 0.001,
            restart_delay_ms: 100..1000,
            ..Default::default()
        }
    }

    /// Create a config focused on partition testing.
    pub fn partition_heavy() -> Self {
        Self {
            base_latency_ms: 10,
            latency_jitter_ms: 5,
            partition_probability: 0.1,
            partition_duration_ms: 200..1000,
            ..Default::default()
        }
    }

    /// Builder: set base latency.
    pub fn with_latency(mut self, base_ms: u64, jitter_ms: u64) -> Self {
        self.base_latency_ms = base_ms;
        self.latency_jitter_ms = jitter_ms;
        self
    }

    /// Builder: set message loss rate.
    pub fn with_loss(mut self, rate: f64) -> Self {
        self.message_loss_rate = rate.clamp(0.0, 1.0);
        self
    }

    /// Builder: set duplicate rate.
    pub fn with_duplicates(mut self, rate: f64) -> Self {
        self.duplicate_rate = rate.clamp(0.0, 1.0);
        self
    }

    /// Builder: set reorder window.
    pub fn with_reorder(mut self, window_ms: u64) -> Self {
        self.reorder_window_ms = window_ms;
        self
    }

    /// Builder: set partition config.
    pub fn with_partitions(mut self, probability: f64, duration_ms: Range<u64>) -> Self {
        self.partition_probability = probability.clamp(0.0, 1.0);
        self.partition_duration_ms = duration_ms;
        self
    }

    /// Builder: set crash config.
    pub fn with_crashes(mut self, probability: f64, restart_delay_ms: Range<u64>) -> Self {
        self.crash_probability = probability.clamp(0.0, 1.0);
        self.restart_delay_ms = restart_delay_ms;
        self
    }

    /// Validate configuration.
    pub fn validate(&self) -> Result<(), String> {
        if !(0.0..=1.0).contains(&self.message_loss_rate) {
            return Err("message_loss_rate must be in [0.0, 1.0]".to_string());
        }
        if !(0.0..=1.0).contains(&self.duplicate_rate) {
            return Err("duplicate_rate must be in [0.0, 1.0]".to_string());
        }
        if !(0.0..=1.0).contains(&self.partition_probability) {
            return Err("partition_probability must be in [0.0, 1.0]".to_string());
        }
        if !(0.0..=1.0).contains(&self.crash_probability) {
            return Err("crash_probability must be in [0.0, 1.0]".to_string());
        }
        if self.slow_node_factor < 0.0 {
            return Err("slow_node_factor must be non-negative".to_string());
        }
        // Prevent overflow when converting to microseconds (ms * 1000)
        // Max safe value is usize::MAX / 1000
        const MAX_REORDER_WINDOW_MS: u64 = (usize::MAX / 1000) as u64;
        if self.reorder_window_ms > MAX_REORDER_WINDOW_MS {
            return Err(format!(
                "reorder_window_ms must be <= {} to prevent overflow",
                MAX_REORDER_WINDOW_MS
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = FaultConfig::default();
        assert!(config.validate().is_ok());
        assert_eq!(config.message_loss_rate, 0.0);
    }

    #[test]
    fn test_none_config() {
        let config = FaultConfig::none();
        assert_eq!(config.base_latency_ms, 0);
        assert_eq!(config.message_loss_rate, 0.0);
    }

    #[test]
    fn test_chaos_configs() {
        let light = FaultConfig::light_chaos();
        assert!(light.validate().is_ok());
        assert!(light.message_loss_rate > 0.0);

        let heavy = FaultConfig::heavy_chaos();
        assert!(heavy.validate().is_ok());
        assert!(heavy.message_loss_rate > light.message_loss_rate);
    }

    #[test]
    fn test_builder_pattern() {
        let config = FaultConfig::none()
            .with_latency(50, 10)
            .with_loss(0.05)
            .with_duplicates(0.02)
            .with_reorder(100);

        assert!(config.validate().is_ok());
        assert_eq!(config.base_latency_ms, 50);
        assert_eq!(config.latency_jitter_ms, 10);
        assert_eq!(config.message_loss_rate, 0.05);
        assert_eq!(config.duplicate_rate, 0.02);
        assert_eq!(config.reorder_window_ms, 100);
    }

    #[test]
    fn test_validation_clamps() {
        // Builder should clamp invalid values
        let config = FaultConfig::none().with_loss(2.0);
        assert_eq!(config.message_loss_rate, 1.0);

        let config = FaultConfig::none().with_loss(-0.5);
        assert_eq!(config.message_loss_rate, 0.0);
    }
}
