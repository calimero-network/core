//! Adapter to use SimMetrics with SyncMetricsCollector trait.
//!
//! This adapter allows the simulation framework to collect metrics using the
//! unified `SyncMetricsCollector` trait while maintaining compatibility with
//! the existing `SimMetrics` structure.
//!
//! # Thread Safety
//!
//! The adapter uses `Mutex` internally to satisfy the `Send + Sync` requirements
//! of `SyncMetricsCollector`. Since simulation runs single-threaded, this adds
//! negligible overhead.

use std::sync::Mutex;
use std::time::Duration;

use calimero_node::sync::metrics::{PhaseTimer, SyncMetricsCollector};

use super::metrics::SimMetrics;

/// Wraps SimMetrics to implement SyncMetricsCollector.
///
/// This adapter bridges the simulation's metrics collection with the unified
/// metrics trait used by production code.
///
/// # Example
///
/// ```rust,ignore
/// use sync_sim::metrics_adapter::SimMetricsCollector;
///
/// let collector = SimMetricsCollector::new();
///
/// // Use as SyncMetricsCollector
/// collector.record_message_sent("HashComparison", 1024);
/// collector.record_round_trip("HashComparison");
///
/// // Get snapshot of metrics
/// let metrics = collector.snapshot();
/// assert_eq!(metrics.protocol.messages_sent, 1);
/// assert_eq!(metrics.protocol.round_trips, 1);
/// ```
#[derive(Debug)]
pub struct SimMetricsCollector {
    metrics: Mutex<SimMetrics>,
}

impl Default for SimMetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl SimMetricsCollector {
    /// Create a new simulation metrics collector.
    pub fn new() -> Self {
        Self {
            metrics: Mutex::new(SimMetrics::default()),
        }
    }

    /// Take ownership of the collected metrics, resetting to default.
    ///
    /// This is useful for extracting metrics after a simulation run.
    pub fn take_metrics(&self) -> SimMetrics {
        let mut guard = self.metrics.lock().expect("metrics lock poisoned");
        std::mem::take(&mut *guard)
    }

    /// Get a snapshot of current metrics without consuming them.
    pub fn snapshot(&self) -> SimMetrics {
        self.metrics.lock().expect("metrics lock poisoned").clone()
    }

    /// Reset all metrics to their default values.
    pub fn reset(&self) {
        let mut guard = self.metrics.lock().expect("metrics lock poisoned");
        guard.reset();
    }

    /// Merge metrics from another SimMetrics instance.
    pub fn merge(&self, other: &SimMetrics) {
        let mut guard = self.metrics.lock().expect("metrics lock poisoned");
        guard.merge(other);
    }
}

impl SyncMetricsCollector for SimMetricsCollector {
    fn record_message_sent(&self, _protocol: &str, bytes: usize) {
        let mut guard = self.metrics.lock().expect("metrics lock poisoned");
        guard.protocol.record_message(bytes);
    }

    fn record_round_trip(&self, _protocol: &str) {
        let mut guard = self.metrics.lock().expect("metrics lock poisoned");
        guard.protocol.record_round_trip();
    }

    fn record_entities_transferred(&self, count: usize) {
        let mut guard = self.metrics.lock().expect("metrics lock poisoned");
        // Use direct increment instead of O(n) loop
        guard.protocol.entities_transferred += count as u64;
    }

    fn record_merge(&self, _crdt_type: &str) {
        let mut guard = self.metrics.lock().expect("metrics lock poisoned");
        guard.protocol.record_merge();
    }

    fn record_comparison(&self) {
        let mut guard = self.metrics.lock().expect("metrics lock poisoned");
        guard.protocol.record_comparison();
    }

    fn record_phase_complete(&self, _timer: PhaseTimer) {
        // SimMetrics doesn't track phase timing directly
        // (uses SimTime for deterministic timing instead)
        // Phase timing is wall-clock based and not suitable for simulation
    }

    fn record_snapshot_blocked(&self) {
        // Could add dedicated counter to SimMetrics if needed
        // For now, this is a safety metric primarily for production monitoring
    }

    fn record_verification_failure(&self) {
        // Could add dedicated counter to SimMetrics if needed
        // For now, this is a safety metric primarily for production monitoring
    }

    fn record_lww_fallback(&self) {
        // Could add dedicated counter to SimMetrics if needed
        // For now, this is a safety metric primarily for production monitoring
    }

    fn record_buffer_drop(&self) {
        let mut guard = self.metrics.lock().expect("metrics lock poisoned");
        guard.effects.record_buffer_drop();
    }

    fn record_sync_start(&self, _context_id: &str, _protocol: &str, _trigger: &str) {
        // Simulation tracks sync sessions differently through the runtime
    }

    fn record_sync_complete(
        &self,
        _context_id: &str,
        _protocol: &str,
        _duration: Duration,
        _entities: usize,
    ) {
        // Simulation uses convergence metrics instead
    }

    fn record_sync_failure(&self, _context_id: &str, _protocol: &str, _reason: &str) {
        // Simulation tracks failures through convergence metrics
    }

    fn record_protocol_selected(&self, _protocol: &str, _reason: &str, _divergence: f64) {
        // Could add protocol selection tracking if needed for benchmarks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collector_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SimMetricsCollector>();
    }

    #[test]
    fn test_record_message_sent() {
        let collector = SimMetricsCollector::new();

        collector.record_message_sent("HashComparison", 1024);
        collector.record_message_sent("HashComparison", 512);

        let metrics = collector.snapshot();
        assert_eq!(metrics.protocol.messages_sent, 2);
        assert_eq!(metrics.protocol.payload_bytes, 1536);
    }

    #[test]
    fn test_record_round_trip() {
        let collector = SimMetricsCollector::new();

        collector.record_round_trip("HashComparison");
        collector.record_round_trip("HashComparison");

        let metrics = collector.snapshot();
        assert_eq!(metrics.protocol.round_trips, 2);
    }

    #[test]
    fn test_record_entities_transferred() {
        let collector = SimMetricsCollector::new();

        collector.record_entities_transferred(5);
        collector.record_entities_transferred(3);

        let metrics = collector.snapshot();
        assert_eq!(metrics.protocol.entities_transferred, 8);
    }

    #[test]
    fn test_record_merge() {
        let collector = SimMetricsCollector::new();

        collector.record_merge("GCounter");
        collector.record_merge("LwwRegister");

        let metrics = collector.snapshot();
        assert_eq!(metrics.protocol.merges_performed, 2);
    }

    #[test]
    fn test_record_comparison() {
        let collector = SimMetricsCollector::new();

        collector.record_comparison();
        collector.record_comparison();
        collector.record_comparison();

        let metrics = collector.snapshot();
        assert_eq!(metrics.protocol.entities_compared, 3);
    }

    #[test]
    fn test_record_buffer_drop() {
        let collector = SimMetricsCollector::new();

        collector.record_buffer_drop();
        collector.record_buffer_drop();

        let metrics = collector.snapshot();
        assert_eq!(metrics.effects.buffer_drops, 2);
    }

    #[test]
    fn test_take_metrics_resets() {
        let collector = SimMetricsCollector::new();

        collector.record_message_sent("test", 100);
        let metrics1 = collector.take_metrics();
        assert_eq!(metrics1.protocol.messages_sent, 1);

        // Should be reset after take
        let metrics2 = collector.snapshot();
        assert_eq!(metrics2.protocol.messages_sent, 0);
    }

    #[test]
    fn test_reset() {
        let collector = SimMetricsCollector::new();

        collector.record_message_sent("test", 100);
        collector.record_round_trip("test");
        collector.reset();

        let metrics = collector.snapshot();
        assert_eq!(metrics.protocol.messages_sent, 0);
        assert_eq!(metrics.protocol.round_trips, 0);
    }
}
