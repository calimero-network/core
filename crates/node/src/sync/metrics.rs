//! Sync protocol metrics collection.
//!
//! This module provides a unified metrics interface for both simulation
//! (deterministic benchmarking) and production (Prometheus observability).
//!
//! # Architecture
//!
//! The `SyncMetricsCollector` trait defines the metrics interface. Implementations:
//! - `NoOpMetrics`: Zero-overhead disabled metrics
//! - `PrometheusSyncMetrics`: Production Prometheus metrics (in `prometheus_metrics` module)
//! - `SimMetricsCollector`: Simulation adapter (in test code)
//!
//! # CIP Invariant Monitoring
//!
//! Safety metrics track potential invariant violations:
//! - I5: `record_snapshot_blocked()` - Snapshot attempts on initialized nodes
//! - I6: `record_buffer_drop()` - Delta buffer overflow events
//! - I7: `record_verification_failure()` - Snapshot verification failures
//!
//! # Usage
//!
//! ```rust,ignore
//! use calimero_node::sync::metrics::{SyncMetricsCollector, PhaseTimer};
//!
//! fn sync_operation(metrics: &dyn SyncMetricsCollector) {
//!     let timer = metrics.start_phase("handshake");
//!     // ... do handshake ...
//!     metrics.record_phase_complete(timer);
//! }
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

/// Phase timer for measuring sync operation phases.
///
/// Created by `SyncMetricsCollector::start_phase()` and consumed by
/// `SyncMetricsCollector::record_phase_complete()`.
#[derive(Debug)]
pub struct PhaseTimer {
    phase: &'static str,
    start: Instant,
}

impl PhaseTimer {
    /// Create a new phase timer.
    pub fn new(phase: &'static str) -> Self {
        Self {
            phase,
            start: Instant::now(),
        }
    }

    /// Get the elapsed time since the timer was created.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Get the phase name.
    pub fn phase(&self) -> &'static str {
        self.phase
    }
}

/// Sync metrics collector trait.
///
/// Implemented by:
/// - `NoOpMetrics` - Zero-overhead disabled metrics
/// - `PrometheusSyncMetrics` - Production Prometheus observability
/// - `SimMetricsCollector` - Simulation/benchmarking adapter
///
/// All methods are designed to be cheap when metrics are disabled.
pub trait SyncMetricsCollector: Send + Sync {
    // =========================================================================
    // Protocol Cost Metrics
    // =========================================================================

    /// Record a sync protocol message sent.
    ///
    /// # Arguments
    /// - `protocol`: Protocol name (e.g., "HashComparison", "Snapshot")
    /// - `bytes`: Size of the message payload in bytes
    fn record_message_sent(&self, protocol: &str, bytes: usize);

    /// Record a request-response round trip.
    ///
    /// A round trip is a complete request-response exchange, counting
    /// latency-sensitive operations for protocol efficiency analysis.
    fn record_round_trip(&self, protocol: &str);

    /// Record entities transferred during sync.
    ///
    /// # Arguments
    /// - `count`: Number of entities (not bytes) transferred
    fn record_entities_transferred(&self, count: usize);

    /// Record a CRDT merge operation.
    ///
    /// # Arguments
    /// - `crdt_type`: The CRDT type being merged (e.g., "GCounter", "LwwRegister")
    fn record_merge(&self, crdt_type: &str);

    /// Record entity hash comparison.
    ///
    /// Counts Merkle tree node comparisons during HashComparison sync.
    fn record_comparison(&self);

    // =========================================================================
    // Phase Timing
    // =========================================================================

    /// Start timing a sync phase.
    ///
    /// Common phase names:
    /// - `protocol_selection`: Time to select sync protocol
    /// - `handshake`: Initial handshake exchange
    /// - `data_transfer`: Bulk data transfer phase
    /// - `merge`: CRDT merge operations
    /// - `sync_total`: Complete sync operation
    fn start_phase(&self, phase: &'static str) -> PhaseTimer {
        PhaseTimer::new(phase)
    }

    /// Record phase completion with duration.
    fn record_phase_complete(&self, timer: PhaseTimer);

    // =========================================================================
    // Safety Metrics (Invariant Monitoring)
    // =========================================================================

    /// Record snapshot blocked on initialized node (I5).
    ///
    /// This metric tracks attempts to perform snapshot sync on a node that
    /// already has state. Such attempts violate Invariant I5 (No Silent Data Loss)
    /// and indicate a bug in protocol selection.
    fn record_snapshot_blocked(&self);

    /// Record snapshot verification failure (I7).
    ///
    /// This metric tracks cases where the received snapshot fails integrity
    /// verification (root hash mismatch). This could indicate data corruption
    /// or a malicious peer.
    fn record_verification_failure(&self);

    /// Record LWW fallback due to missing crdt_type.
    ///
    /// This metric tracks cases where CRDT merge falls back to Last-Write-Wins
    /// because the entity lacks `crdt_type` metadata (legacy data migration).
    fn record_lww_fallback(&self);

    /// Record delta buffer drop (I6 risk).
    ///
    /// This metric tracks delta buffer overflow events where incoming deltas
    /// are dropped due to buffer capacity limits. This is an I6 violation risk
    /// that could lead to divergence.
    fn record_buffer_drop(&self);

    // =========================================================================
    // Sync Session Lifecycle
    // =========================================================================

    /// Record sync session start.
    ///
    /// # Arguments
    /// - `context_id`: The context being synced
    /// - `protocol`: The selected protocol
    /// - `trigger`: What triggered the sync ("timer", "divergence", "manual")
    fn record_sync_start(&self, context_id: &str, protocol: &str, trigger: &str);

    /// Record successful sync completion.
    ///
    /// # Arguments
    /// - `context_id`: The context that was synced
    /// - `duration`: Total sync duration
    /// - `entities`: Number of entities synced
    fn record_sync_complete(&self, context_id: &str, duration: Duration, entities: usize);

    /// Record sync failure.
    ///
    /// # Arguments
    /// - `context_id`: The context that failed to sync
    /// - `reason`: Human-readable failure reason
    fn record_sync_failure(&self, context_id: &str, reason: &str);

    // =========================================================================
    // Protocol Selection
    // =========================================================================

    /// Record protocol selection decision.
    ///
    /// # Arguments
    /// - `protocol`: The selected protocol name
    /// - `reason`: Why this protocol was selected
    /// - `divergence`: Estimated divergence percentage (0.0-1.0)
    fn record_protocol_selected(&self, protocol: &str, reason: &str, divergence: f64);
}

/// No-op implementation for when metrics are disabled.
///
/// All methods are inline no-ops, allowing the compiler to eliminate
/// metrics calls entirely when using this implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoOpMetrics;

impl SyncMetricsCollector for NoOpMetrics {
    #[inline]
    fn record_message_sent(&self, _protocol: &str, _bytes: usize) {}

    #[inline]
    fn record_round_trip(&self, _protocol: &str) {}

    #[inline]
    fn record_entities_transferred(&self, _count: usize) {}

    #[inline]
    fn record_merge(&self, _crdt_type: &str) {}

    #[inline]
    fn record_comparison(&self) {}

    #[inline]
    fn record_phase_complete(&self, _timer: PhaseTimer) {}

    #[inline]
    fn record_snapshot_blocked(&self) {}

    #[inline]
    fn record_verification_failure(&self) {}

    #[inline]
    fn record_lww_fallback(&self) {}

    #[inline]
    fn record_buffer_drop(&self) {}

    #[inline]
    fn record_sync_start(&self, _context_id: &str, _protocol: &str, _trigger: &str) {}

    #[inline]
    fn record_sync_complete(&self, _context_id: &str, _duration: Duration, _entities: usize) {}

    #[inline]
    fn record_sync_failure(&self, _context_id: &str, _reason: &str) {}

    #[inline]
    fn record_protocol_selected(&self, _protocol: &str, _reason: &str, _divergence: f64) {}
}

/// Type alias for a shared metrics collector.
pub type SharedMetrics = Arc<dyn SyncMetricsCollector>;

/// Create a no-op metrics instance.
///
/// Use this when metrics are disabled or not needed.
pub fn no_op_metrics() -> Arc<NoOpMetrics> {
    Arc::new(NoOpMetrics)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase_timer() {
        let timer = PhaseTimer::new("test_phase");
        assert_eq!(timer.phase(), "test_phase");

        // Timer should have non-zero elapsed time after creation
        std::thread::sleep(std::time::Duration::from_millis(1));
        assert!(timer.elapsed() >= std::time::Duration::from_millis(1));
    }

    #[test]
    fn test_no_op_metrics_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NoOpMetrics>();
    }

    #[test]
    fn test_no_op_metrics_all_methods() {
        let metrics = NoOpMetrics;

        // All methods should work without panic
        metrics.record_message_sent("test", 100);
        metrics.record_round_trip("test");
        metrics.record_entities_transferred(10);
        metrics.record_merge("GCounter");
        metrics.record_comparison();

        let timer = metrics.start_phase("test");
        metrics.record_phase_complete(timer);

        metrics.record_snapshot_blocked();
        metrics.record_verification_failure();
        metrics.record_lww_fallback();
        metrics.record_buffer_drop();

        metrics.record_sync_start("ctx-123", "HashComparison", "timer");
        metrics.record_sync_complete("ctx-123", Duration::from_secs(1), 50);
        metrics.record_sync_failure("ctx-123", "timeout");
        metrics.record_protocol_selected("HashComparison", "divergence < 10%", 0.05);
    }
}
