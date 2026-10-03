//! Prometheus-based sync metrics implementation.
//!
//! Exports only what a production code path records. The
//! [`SyncMetricsCollector`] trait also carries per-message, per-merge and
//! per-phase hooks that the sync simulator (`crates/node/tests/sync_sim`)
//! drives; no production call site feeds them, so they are not registered
//! here. A series that is registered but never written reads a flat 0, which
//! a dashboard cannot tell apart from "nothing went wrong".
//!
//! # Metric Categories
//!
//! ## Safety Metrics (Invariant Monitoring)
//! - `sync_snapshot_blocked_total`: Snapshot attempts blocked on an
//!   initialised node (I5)
//! - `sync_verification_failures_total`: Snapshot root-hash verification
//!   failures (I7)
//!
//! Delta-buffer drops (I6) are counted as `sync_buffer_drops_total` by
//! `node_metrics`, because the drop happens in `NodeState`, which has no
//! handle to this collector.
//!
//! ## Sync Session Metrics
//! - `sync_duration_seconds{protocol,outcome}`: Session duration histogram,
//!   successes and failures
//! - `sync_attempts_total{protocol}`: Total sync attempts
//! - `sync_successes_total{protocol}`: Successful syncs
//! - `sync_failures_total{protocol}`: Failed syncs
//! - `sync_protocol_selections_total{protocol}`: Adaptive selector decisions

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::histogram::{exponential_buckets, Histogram};
use prometheus_client::registry::Registry;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use super::metrics::{PhaseTimer, SyncMetricsCollector};

/// Known sync protocol names for label sanitization.
///
/// This prevents unbounded label cardinality from untrusted input.
const KNOWN_PROTOCOLS: &[&str] = &[
    "None",
    "Snapshot",
    "HashComparison",
    "DeltaSync",
    "SubtreePrefetch",
    "LevelWise",
    "BloomFilter",
];

/// Sanitize a protocol name to prevent unbounded label cardinality.
///
/// Returns the protocol name if known, otherwise "unknown".
fn sanitize_protocol(protocol: &str) -> &'static str {
    KNOWN_PROTOCOLS
        .iter()
        .find(|&&p| p == protocol)
        .copied()
        .unwrap_or("unknown")
}

/// Labels for protocol-specific metrics.
///
/// Label values are `&'static str` sourced from the `sanitize_*` allow-lists
/// (or other compile-time-known strings), so recording a metric never allocates.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ProtocolLabels {
    protocol: &'static str,
}

/// Labels for sync outcome metrics.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct OutcomeLabels {
    protocol: &'static str,
    outcome: &'static str,
}

/// Prometheus-based sync metrics collector.
///
/// Register this with your Prometheus registry during node initialization.
/// All metrics are thread-safe and use atomic operations.
#[derive(Debug)]
pub struct PrometheusSyncMetrics {
    // Safety metrics
    snapshot_blocked_total: Counter<u64, AtomicU64>,
    verification_failures_total: Counter<u64, AtomicU64>,

    // Sync session metrics
    sync_duration_seconds: Family<OutcomeLabels, Histogram>,
    sync_attempts_total: Family<ProtocolLabels, Counter>,
    sync_successes_total: Family<ProtocolLabels, Counter>,
    sync_failures_total: Family<ProtocolLabels, Counter>,

    // Protocol selection metrics
    protocol_selections_total: Family<ProtocolLabels, Counter>,
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
        let metrics = Self {
            snapshot_blocked_total: Counter::default(),
            verification_failures_total: Counter::default(),
            sync_duration_seconds: Family::new_with_constructor(|| {
                // Buckets from 10ms to ~160s (15 exponential buckets, base 2)
                Histogram::new(exponential_buckets(0.01, 2.0, 15))
            }),
            sync_attempts_total: Family::default(),
            sync_successes_total: Family::default(),
            sync_failures_total: Family::default(),
            protocol_selections_total: Family::default(),
        };

        registry.register(
            "sync_snapshot_blocked",
            "Snapshot attempts blocked on initialized nodes (I5 protection)",
            metrics.snapshot_blocked_total.clone(),
        );
        registry.register(
            "sync_verification_failures",
            "Snapshot root-hash verification failures (I7 violations)",
            metrics.verification_failures_total.clone(),
        );
        registry.register(
            "sync_duration_seconds",
            "Duration of sync sessions in seconds, by protocol and outcome (success / failure)",
            metrics.sync_duration_seconds.clone(),
        );
        registry.register(
            "sync_attempts",
            "Total sync attempts (protocol is not known yet when an attempt starts, so labelled unknown)",
            metrics.sync_attempts_total.clone(),
        );
        registry.register(
            "sync_successes",
            "Total successful syncs by protocol",
            metrics.sync_successes_total.clone(),
        );
        registry.register(
            "sync_failures",
            "Total failed syncs (labelled unknown: an attempt can fail before a protocol is chosen)",
            metrics.sync_failures_total.clone(),
        );
        registry.register(
            "sync_protocol_selections",
            "Total protocol selection decisions by protocol",
            metrics.protocol_selections_total.clone(),
        );

        metrics
    }
}

/// The per-message, per-merge, per-phase and LWW-fallback hooks have no
/// production call site (see the module docs), so this collector ignores them
/// rather than exporting series that would only ever read 0. Wire a call site
/// and register a series together.
impl SyncMetricsCollector for PrometheusSyncMetrics {
    fn record_message_sent(&self, _protocol: &str, _bytes: usize) {}

    fn record_round_trip(&self, _protocol: &str) {}

    fn record_entities_transferred(&self, _count: usize) {}

    fn record_merge(&self, _crdt_type: &str) {}

    fn record_comparison(&self) {}

    fn record_phase_complete(&self, _timer: PhaseTimer) {}

    fn record_snapshot_blocked(&self) {
        self.snapshot_blocked_total.inc();
    }

    fn record_verification_failure(&self) {
        self.verification_failures_total.inc();
    }

    fn record_lww_fallback(&self) {}

    /// Counted by `node_metrics::record_sync_buffer_drop` instead.
    fn record_buffer_drop(&self) {}

    fn record_sync_start(&self, _context_id: &str, protocol: &str, _trigger: &str) {
        let labels = ProtocolLabels {
            protocol: sanitize_protocol(protocol),
        };
        self.sync_attempts_total.get_or_create(&labels).inc();
    }

    fn record_sync_complete(
        &self,
        _context_id: &str,
        protocol: &str,
        duration: Duration,
        _entities: usize,
    ) {
        let sanitized = sanitize_protocol(protocol);
        let labels = OutcomeLabels {
            protocol: sanitized,
            outcome: "success",
        };
        self.sync_duration_seconds
            .get_or_create(&labels)
            .observe(duration.as_secs_f64());

        // Increment success counter
        let success_labels = ProtocolLabels {
            protocol: sanitized,
        };
        self.sync_successes_total
            .get_or_create(&success_labels)
            .inc();
    }

    fn record_sync_failure(
        &self,
        _context_id: &str,
        protocol: &str,
        duration: Duration,
        _reason: &str,
    ) {
        let sanitized = sanitize_protocol(protocol);
        // Failed attempts go into the duration histogram too, under
        // `outcome="failure"`: a sync that times out is exactly the slow tail
        // a p99 panel exists to show, and leaving it out made the histogram
        // look healthiest while syncs were failing.
        self.sync_duration_seconds
            .get_or_create(&OutcomeLabels {
                protocol: sanitized,
                outcome: "failure",
            })
            .observe(duration.as_secs_f64());
        self.sync_failures_total
            .get_or_create(&ProtocolLabels {
                protocol: sanitized,
            })
            .inc();
    }

    fn record_protocol_selected(&self, protocol: &str, _reason: &str, _divergence: f64) {
        let labels = ProtocolLabels {
            protocol: sanitize_protocol(protocol),
        };
        self.protocol_selections_total.get_or_create(&labels).inc();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(registry: &Registry) -> String {
        let mut buffer = String::new();
        prometheus_client::encoding::text::encode(&mut buffer, registry).unwrap();
        buffer
    }

    #[test]
    fn test_prometheus_metrics_creation() {
        let mut registry = Registry::default();
        let _metrics = PrometheusSyncMetrics::new(&mut registry);
        let buffer = encoded(&registry);

        assert!(buffer.contains("sync_snapshot_blocked"));
        assert!(buffer.contains("sync_verification_failures"));
        // Never written in production, so never exported.
        for absent in [
            "sync_messages_sent",
            "sync_bytes_sent",
            "sync_round_trips",
            "sync_entities_transferred",
            "sync_merges",
            "sync_comparisons",
            "sync_phase_duration_seconds",
            "sync_lww_fallback",
        ] {
            assert!(!buffer.contains(absent), "{absent} exported:\n{buffer}");
        }
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

        metrics.record_snapshot_blocked();
        metrics.record_verification_failure();
        metrics.record_sync_start("ctx-123", "HashComparison", "timer");
        metrics.record_sync_complete("ctx-123", "HashComparison", Duration::from_millis(100), 50);
        metrics.record_sync_failure("ctx-456", "Snapshot", Duration::from_secs(30), "timeout");
        metrics.record_protocol_selected("HashComparison", "test", 0.05);

        let buffer = encoded(&registry);
        assert!(buffer.contains("sync_snapshot_blocked_total 1"), "{buffer}");
        assert!(
            buffer.contains("sync_verification_failures_total 1"),
            "{buffer}"
        );
        assert!(
            buffer.contains(
                "sync_duration_seconds_count{protocol=\"Snapshot\",outcome=\"failure\"} 1"
            ),
            "failed sync missing from the duration histogram:\n{buffer}"
        );
        assert!(
            buffer.contains("sync_protocol_selections_total{protocol=\"HashComparison\"} 1"),
            "{buffer}"
        );
    }
}
