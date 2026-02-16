//! Prometheus-based sync metrics implementation.
//!
//! Provides production-grade observability for the sync protocol using
//! the `prometheus-client` crate (already in dependencies).
//!
//! # Metric Categories
//!
//! ## Protocol Cost Metrics
//! - `sync_messages_sent_total{protocol}`: Messages sent by protocol type
//! - `sync_bytes_sent_total{protocol}`: Bytes sent by protocol type
//! - `sync_round_trips_total{protocol}`: Round trips by protocol type
//! - `sync_entities_transferred_total`: Total entities transferred
//! - `sync_merges_total{crdt_type}`: CRDT merges by type
//! - `sync_comparisons_total`: Hash comparisons performed
//!
//! ## Phase Timing
//! - `sync_phase_duration_seconds{phase}`: Histogram of phase durations
//!
//! ## Safety Metrics (Invariant Monitoring)
//! - `sync_snapshot_blocked_total`: Snapshot attempts blocked (I5)
//! - `sync_verification_failures_total`: Verification failures (I7)
//! - `sync_lww_fallback_total`: LWW fallback events
//! - `sync_buffer_drops_total`: Delta buffer drops (I6)
//!
//! ## Sync Session Metrics
//! - `sync_duration_seconds{protocol,outcome}`: Session duration histogram
//! - `sync_attempts_total{protocol}`: Total sync attempts
//! - `sync_successes_total{protocol}`: Successful syncs
//! - `sync_failures_total{protocol}`: Failed syncs

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::histogram::{exponential_buckets, Histogram};
use prometheus_client::registry::Registry;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use super::metrics::{PhaseTimer, SyncMetricsCollector};

/// Labels for protocol-specific metrics.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ProtocolLabels {
    protocol: String,
}

/// Labels for CRDT type metrics.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct CrdtLabels {
    crdt_type: String,
}

/// Labels for phase timing metrics.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct PhaseLabels {
    phase: String,
}

/// Labels for sync outcome metrics.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct OutcomeLabels {
    protocol: String,
    outcome: String,
}

/// Prometheus-based sync metrics collector.
///
/// Register this with your Prometheus registry during node initialization.
/// All metrics are thread-safe and use atomic operations.
#[derive(Debug)]
pub struct PrometheusSyncMetrics {
    // Protocol cost metrics
    messages_sent: Family<ProtocolLabels, Counter>,
    bytes_sent: Family<ProtocolLabels, Counter>,
    round_trips: Family<ProtocolLabels, Counter>,
    entities_transferred: Counter<u64, AtomicU64>,
    merges_total: Family<CrdtLabels, Counter>,
    comparisons_total: Counter<u64, AtomicU64>,

    // Phase timing
    phase_duration_seconds: Family<PhaseLabels, Histogram>,

    // Safety metrics
    snapshot_blocked_total: Counter<u64, AtomicU64>,
    verification_failures_total: Counter<u64, AtomicU64>,
    lww_fallback_total: Counter<u64, AtomicU64>,
    buffer_drops_total: Counter<u64, AtomicU64>,

    // Sync session metrics
    sync_duration_seconds: Family<OutcomeLabels, Histogram>,
    sync_attempts_total: Family<ProtocolLabels, Counter>,
    sync_successes_total: Family<ProtocolLabels, Counter>,
    sync_failures_total: Family<ProtocolLabels, Counter>,
}

impl PrometheusSyncMetrics {
    /// Create and register sync metrics with a Prometheus registry.
    ///
    /// # Arguments
    /// - `registry`: The Prometheus registry to register metrics with
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use prometheus_client::registry::Registry;
    /// use calimero_node::sync::prometheus_metrics::PrometheusSyncMetrics;
    ///
    /// let mut registry = Registry::default();
    /// let metrics = PrometheusSyncMetrics::new(&mut registry);
    /// ```
    pub fn new(registry: &mut Registry) -> Self {
        // Create metrics with sensible histogram buckets
        let metrics = Self {
            messages_sent: Family::default(),
            bytes_sent: Family::default(),
            round_trips: Family::default(),
            entities_transferred: Counter::default(),
            merges_total: Family::default(),
            comparisons_total: Counter::default(),
            phase_duration_seconds: Family::new_with_constructor(|| {
                // Buckets from 1ms to ~16s (15 exponential buckets, base 2)
                Histogram::new(exponential_buckets(0.001, 2.0, 15))
            }),
            snapshot_blocked_total: Counter::default(),
            verification_failures_total: Counter::default(),
            lww_fallback_total: Counter::default(),
            buffer_drops_total: Counter::default(),
            sync_duration_seconds: Family::new_with_constructor(|| {
                // Buckets from 10ms to ~160s (15 exponential buckets, base 2)
                Histogram::new(exponential_buckets(0.01, 2.0, 15))
            }),
            sync_attempts_total: Family::default(),
            sync_successes_total: Family::default(),
            sync_failures_total: Family::default(),
        };

        // Register all metrics with descriptions
        registry.register(
            "sync_messages_sent",
            "Total sync protocol messages sent",
            metrics.messages_sent.clone(),
        );
        registry.register(
            "sync_bytes_sent",
            "Total sync protocol bytes sent",
            metrics.bytes_sent.clone(),
        );
        registry.register(
            "sync_round_trips",
            "Total sync round trips",
            metrics.round_trips.clone(),
        );
        registry.register(
            "sync_entities_transferred",
            "Total entities transferred during sync",
            metrics.entities_transferred.clone(),
        );
        registry.register(
            "sync_merges",
            "Total CRDT merge operations",
            metrics.merges_total.clone(),
        );
        registry.register(
            "sync_comparisons",
            "Total entity hash comparisons",
            metrics.comparisons_total.clone(),
        );
        registry.register(
            "sync_phase_duration_seconds",
            "Duration of sync phases in seconds",
            metrics.phase_duration_seconds.clone(),
        );
        registry.register(
            "sync_snapshot_blocked",
            "Snapshot attempts blocked on initialized nodes (I5 protection)",
            metrics.snapshot_blocked_total.clone(),
        );
        registry.register(
            "sync_verification_failures",
            "Snapshot verification failures (I7 violations)",
            metrics.verification_failures_total.clone(),
        );
        registry.register(
            "sync_lww_fallback",
            "LWW fallback events due to missing CRDT type metadata",
            metrics.lww_fallback_total.clone(),
        );
        registry.register(
            "sync_buffer_drops",
            "Delta buffer drop events (I6 violation risk)",
            metrics.buffer_drops_total.clone(),
        );
        registry.register(
            "sync_duration_seconds",
            "Duration of sync sessions in seconds",
            metrics.sync_duration_seconds.clone(),
        );
        registry.register(
            "sync_attempts",
            "Total sync attempts by protocol",
            metrics.sync_attempts_total.clone(),
        );
        registry.register(
            "sync_successes",
            "Total successful syncs by protocol",
            metrics.sync_successes_total.clone(),
        );
        registry.register(
            "sync_failures",
            "Total failed syncs by protocol",
            metrics.sync_failures_total.clone(),
        );

        metrics
    }
}

impl SyncMetricsCollector for PrometheusSyncMetrics {
    fn record_message_sent(&self, protocol: &str, bytes: usize) {
        let labels = ProtocolLabels {
            protocol: protocol.to_string(),
        };
        self.messages_sent.get_or_create(&labels).inc();
        self.bytes_sent.get_or_create(&labels).inc_by(bytes as u64);
    }

    fn record_round_trip(&self, protocol: &str) {
        let labels = ProtocolLabels {
            protocol: protocol.to_string(),
        };
        self.round_trips.get_or_create(&labels).inc();
    }

    fn record_entities_transferred(&self, count: usize) {
        self.entities_transferred.inc_by(count as u64);
    }

    fn record_merge(&self, crdt_type: &str) {
        let labels = CrdtLabels {
            crdt_type: crdt_type.to_string(),
        };
        self.merges_total.get_or_create(&labels).inc();
    }

    fn record_comparison(&self) {
        self.comparisons_total.inc();
    }

    fn record_phase_complete(&self, timer: PhaseTimer) {
        let labels = PhaseLabels {
            phase: timer.phase().to_string(),
        };
        self.phase_duration_seconds
            .get_or_create(&labels)
            .observe(timer.elapsed().as_secs_f64());
    }

    fn record_snapshot_blocked(&self) {
        self.snapshot_blocked_total.inc();
    }

    fn record_verification_failure(&self) {
        self.verification_failures_total.inc();
    }

    fn record_lww_fallback(&self) {
        self.lww_fallback_total.inc();
    }

    fn record_buffer_drop(&self) {
        self.buffer_drops_total.inc();
    }

    fn record_sync_start(&self, _context_id: &str, protocol: &str, _trigger: &str) {
        let labels = ProtocolLabels {
            protocol: protocol.to_string(),
        };
        self.sync_attempts_total.get_or_create(&labels).inc();
    }

    fn record_sync_complete(&self, _context_id: &str, duration: Duration, _entities: usize) {
        let labels = OutcomeLabels {
            protocol: "all".to_string(),
            outcome: "success".to_string(),
        };
        self.sync_duration_seconds
            .get_or_create(&labels)
            .observe(duration.as_secs_f64());
    }

    fn record_sync_failure(&self, _context_id: &str, _reason: &str) {
        let labels = ProtocolLabels {
            protocol: "all".to_string(),
        };
        self.sync_failures_total.get_or_create(&labels).inc();
    }

    fn record_protocol_selected(&self, _protocol: &str, _reason: &str, _divergence: f64) {
        // Protocol selection is logged, not metered
        // Could add a counter per protocol if needed for analysis
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prometheus_metrics_creation() {
        let mut registry = Registry::default();
        let _metrics = PrometheusSyncMetrics::new(&mut registry);

        // Verify metrics are registered by encoding
        let mut buffer = String::new();
        prometheus_client::encoding::text::encode(&mut buffer, &registry).unwrap();

        // Check that some expected metrics are present
        assert!(buffer.contains("sync_messages_sent"));
        assert!(buffer.contains("sync_snapshot_blocked"));
        assert!(buffer.contains("sync_buffer_drops"));
    }

    #[test]
    fn test_prometheus_metrics_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PrometheusSyncMetrics>();
    }

    #[test]
    fn test_prometheus_metrics_recording() {
        let mut registry = Registry::default();
        let metrics = PrometheusSyncMetrics::new(&mut registry);

        // Record some metrics
        metrics.record_message_sent("HashComparison", 1024);
        metrics.record_round_trip("HashComparison");
        metrics.record_entities_transferred(10);
        metrics.record_merge("GCounter");
        metrics.record_comparison();
        metrics.record_snapshot_blocked();
        metrics.record_verification_failure();
        metrics.record_lww_fallback();
        metrics.record_buffer_drop();

        let timer = metrics.start_phase("test_phase");
        std::thread::sleep(std::time::Duration::from_millis(1));
        metrics.record_phase_complete(timer);

        metrics.record_sync_start("ctx-123", "HashComparison", "timer");
        metrics.record_sync_complete("ctx-123", Duration::from_millis(100), 50);
        metrics.record_sync_failure("ctx-456", "timeout");
        metrics.record_protocol_selected("HashComparison", "test", 0.05);

        // Encode and verify non-empty
        let mut buffer = String::new();
        prometheus_client::encoding::text::encode(&mut buffer, &registry).unwrap();
        assert!(!buffer.is_empty());
    }
}
