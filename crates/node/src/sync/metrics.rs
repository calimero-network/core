//! Prometheus metrics for synchronization operations.
//!
//! This module provides observability into sync performance and behavior:
//! - Duration histograms for each sync protocol
//! - Per-phase timing breakdown (peer_discovery, key_share, data_transfer, merge)
//! - Success/failure counters
//! - Active sync gauge
//! - Records and bytes transferred

use std::sync::Arc;
use std::time::Instant;

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{exponential_buckets, Histogram};
use prometheus_client::registry::Registry;

// =============================================================================
// Per-Phase Timing (Critical for Root Cause Analysis)
// =============================================================================

/// Detailed timing breakdown for a single sync operation.
///
/// This struct captures per-phase timing to enable root cause analysis
/// of tail latency. Without this, we can only observe total duration
/// but cannot attribute it to specific phases.
#[derive(Debug, Clone, Default)]
pub struct SyncPhaseTimings {
    /// Time to select and connect to peer (ms)
    pub peer_selection_ms: f64,
    /// Time for key share handshake (ms)
    pub key_share_ms: f64,
    /// Time for DAG state comparison (ms)
    pub dag_compare_ms: f64,
    /// Time for data transfer (snapshot pages or deltas) (ms)
    pub data_transfer_ms: f64,
    /// Time waiting for timeout on unresponsive peer (ms)
    pub timeout_wait_ms: f64,
    /// Time for merge operations (ms)
    pub merge_ms: f64,
    /// Number of merge operations performed
    pub merge_count: u64,
    /// Number of hash comparisons performed
    pub hash_compare_count: u64,
    /// Total bytes received
    pub bytes_received: u64,
    /// Total bytes sent
    pub bytes_sent: u64,
    /// Total duration (ms) - sum of all phases
    pub total_ms: f64,
}

impl SyncPhaseTimings {
    /// Create a new timing tracker starting now.
    pub fn new() -> Self {
        Self::default()
    }

    /// Log the phase breakdown for analysis.
    pub fn log(&self, context_id: &str, peer_id: &str, protocol: &str) {
        tracing::info!(
            %context_id,
            %peer_id,
            %protocol,
            peer_selection_ms = format!("{:.2}", self.peer_selection_ms),
            key_share_ms = format!("{:.2}", self.key_share_ms),
            dag_compare_ms = format!("{:.2}", self.dag_compare_ms),
            data_transfer_ms = format!("{:.2}", self.data_transfer_ms),
            timeout_wait_ms = format!("{:.2}", self.timeout_wait_ms),
            merge_ms = format!("{:.2}", self.merge_ms),
            merge_count = self.merge_count,
            hash_compare_count = self.hash_compare_count,
            bytes_received = self.bytes_received,
            bytes_sent = self.bytes_sent,
            total_ms = format!("{:.2}", self.total_ms),
            "SYNC_PHASE_BREAKDOWN"  // Unique marker for log parsing
        );
    }
}

/// Helper to time individual phases.
pub struct PhaseTimer {
    start: Instant,
}

impl PhaseTimer {
    /// Start timing a phase.
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Stop timing and return elapsed milliseconds.
    pub fn stop(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }
}

/// Labels for sync protocol metrics.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct SyncProtocolLabel {
    /// The sync protocol type (snapshot, delta, dag_catchup, hash_comparison, etc.)
    pub protocol: String,
}

/// Labels for sync outcome metrics.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct SyncOutcomeLabel {
    /// The sync protocol type
    pub protocol: String,
    /// The outcome (success, failure, timeout)
    pub outcome: String,
}

/// Prometheus metrics for sync operations.
#[derive(Debug, Clone)]
pub struct SyncMetrics {
    /// Duration of sync operations (seconds), bucketed by protocol.
    /// Buckets: 10ms to 5 minutes
    pub sync_duration: Histogram,

    /// Total number of sync attempts, labeled by protocol.
    pub sync_attempts: Counter,

    /// Total number of successful syncs, labeled by protocol.
    pub sync_successes: Counter,

    /// Total number of failed syncs, labeled by protocol and reason.
    pub sync_failures: Counter,

    /// Currently active sync operations.
    pub active_syncs: Gauge,

    /// Total records applied during snapshot sync.
    pub snapshot_records_applied: Counter,

    /// Total bytes received during sync (uncompressed).
    pub bytes_received: Counter,

    /// Total bytes sent during sync (uncompressed).
    pub bytes_sent: Counter,

    /// Total delta operations fetched.
    pub deltas_fetched: Counter,

    /// Total delta operations applied.
    pub deltas_applied: Counter,

    // =========================================================================
    // Per-Phase Timing Histograms (for root cause analysis)
    // =========================================================================
    /// Time spent in peer selection phase (seconds).
    pub phase_peer_selection: Histogram,

    /// Time spent in key share phase (seconds).
    pub phase_key_share: Histogram,

    /// Time spent in DAG comparison phase (seconds).
    pub phase_dag_compare: Histogram,

    /// Time spent in data transfer phase (seconds).
    pub phase_data_transfer: Histogram,

    /// Time spent waiting for timeouts (seconds).
    pub phase_timeout_wait: Histogram,

    /// Time spent in merge operations (seconds).
    pub phase_merge: Histogram,

    /// Number of merge operations per sync.
    pub merge_operations: Counter,

    /// Number of hash comparisons per sync.
    pub hash_comparisons: Counter,
}

impl SyncMetrics {
    /// Create new metrics and register with the provided registry.
    pub fn new(registry: &mut Registry) -> Self {
        // Duration buckets: 10ms, 25ms, 50ms, 100ms, 250ms, 500ms, 1s, 2.5s, 5s, 10s, 30s, 60s, 120s, 300s
        let sync_duration = Histogram::new(exponential_buckets(0.01, 2.5, 14));

        let sync_attempts = Counter::default();
        let sync_successes = Counter::default();
        let sync_failures = Counter::default();
        let active_syncs = Gauge::default();
        let snapshot_records_applied = Counter::default();
        let bytes_received = Counter::default();
        let bytes_sent = Counter::default();
        let deltas_fetched = Counter::default();
        let deltas_applied = Counter::default();

        // Per-phase timing histograms (smaller buckets for finer granularity)
        // Buckets: 1ms, 2ms, 5ms, 10ms, 25ms, 50ms, 100ms, 250ms, 500ms, 1s, 2.5s, 5s, 10s
        let phase_peer_selection = Histogram::new(exponential_buckets(0.001, 2.5, 13));
        let phase_key_share = Histogram::new(exponential_buckets(0.001, 2.5, 13));
        let phase_dag_compare = Histogram::new(exponential_buckets(0.001, 2.5, 13));
        let phase_data_transfer = Histogram::new(exponential_buckets(0.001, 2.5, 13));
        let phase_timeout_wait = Histogram::new(exponential_buckets(0.001, 2.5, 13));
        let phase_merge = Histogram::new(exponential_buckets(0.001, 2.5, 13));

        let merge_operations = Counter::default();
        let hash_comparisons = Counter::default();

        let sub_registry = registry.sub_registry_with_prefix("sync");

        sub_registry.register(
            "duration_seconds",
            "Duration of sync operations in seconds",
            sync_duration.clone(),
        );

        sub_registry.register(
            "attempts_total",
            "Total number of sync attempts",
            sync_attempts.clone(),
        );

        sub_registry.register(
            "successes_total",
            "Total number of successful sync operations",
            sync_successes.clone(),
        );

        sub_registry.register(
            "failures_total",
            "Total number of failed sync operations",
            sync_failures.clone(),
        );

        sub_registry.register(
            "active",
            "Currently active sync operations",
            active_syncs.clone(),
        );

        sub_registry.register(
            "snapshot_records_applied_total",
            "Total records applied during snapshot syncs",
            snapshot_records_applied.clone(),
        );

        sub_registry.register(
            "bytes_received_total",
            "Total bytes received during sync (uncompressed)",
            bytes_received.clone(),
        );

        sub_registry.register(
            "bytes_sent_total",
            "Total bytes sent during sync (uncompressed)",
            bytes_sent.clone(),
        );

        sub_registry.register(
            "deltas_fetched_total",
            "Total delta operations fetched from peers",
            deltas_fetched.clone(),
        );

        sub_registry.register(
            "deltas_applied_total",
            "Total delta operations applied",
            deltas_applied.clone(),
        );

        // Register per-phase timing histograms
        sub_registry.register(
            "phase_peer_selection_seconds",
            "Time spent selecting and connecting to peer",
            phase_peer_selection.clone(),
        );

        sub_registry.register(
            "phase_key_share_seconds",
            "Time spent in key share handshake",
            phase_key_share.clone(),
        );

        sub_registry.register(
            "phase_dag_compare_seconds",
            "Time spent comparing DAG state",
            phase_dag_compare.clone(),
        );

        sub_registry.register(
            "phase_data_transfer_seconds",
            "Time spent transferring data (snapshots or deltas)",
            phase_data_transfer.clone(),
        );

        sub_registry.register(
            "phase_timeout_wait_seconds",
            "Time spent waiting for peer timeouts",
            phase_timeout_wait.clone(),
        );

        sub_registry.register(
            "phase_merge_seconds",
            "Time spent in merge operations",
            phase_merge.clone(),
        );

        sub_registry.register(
            "merge_operations_total",
            "Total number of merge operations performed",
            merge_operations.clone(),
        );

        sub_registry.register(
            "hash_comparisons_total",
            "Total number of hash comparisons performed",
            hash_comparisons.clone(),
        );

        Self {
            sync_duration,
            sync_attempts,
            sync_successes,
            sync_failures,
            active_syncs,
            snapshot_records_applied,
            bytes_received,
            bytes_sent,
            deltas_fetched,
            deltas_applied,
            phase_peer_selection,
            phase_key_share,
            phase_dag_compare,
            phase_data_transfer,
            phase_timeout_wait,
            phase_merge,
            merge_operations,
            hash_comparisons,
        }
    }

    /// Create a guard that tracks sync duration and outcome.
    pub fn start_sync(&self, protocol: &str) -> SyncTimingGuard {
        self.sync_attempts.inc();
        self.active_syncs.inc();

        SyncTimingGuard {
            metrics: self.clone(),
            protocol: protocol.to_string(),
            start: Instant::now(),
            completed: false,
        }
    }

    /// Record sync completion (called by SyncTimingGuard).
    fn record_completion(&self, protocol: &str, duration_secs: f64, success: bool) {
        self.sync_duration.observe(duration_secs);
        self.active_syncs.dec();

        if success {
            self.sync_successes.inc();
        } else {
            self.sync_failures.inc();
        }

        tracing::debug!(
            protocol,
            duration_ms = format!("{:.2}", duration_secs * 1000.0),
            success,
            "Sync operation completed"
        );
    }

    /// Record snapshot records applied.
    pub fn record_snapshot_records(&self, count: u64) {
        self.snapshot_records_applied.inc_by(count);
    }

    /// Record bytes received.
    pub fn record_bytes_received(&self, bytes: u64) {
        self.bytes_received.inc_by(bytes);
    }

    /// Record bytes sent.
    pub fn record_bytes_sent(&self, bytes: u64) {
        self.bytes_sent.inc_by(bytes);
    }

    /// Record deltas fetched from peers.
    pub fn record_deltas_fetched(&self, count: u64) {
        self.deltas_fetched.inc_by(count);
    }

    /// Record deltas applied.
    pub fn record_deltas_applied(&self, count: u64) {
        self.deltas_applied.inc_by(count);
    }

    /// Record per-phase timing breakdown.
    ///
    /// This is critical for root cause analysis of tail latency.
    pub fn record_phase_timings(&self, timings: &SyncPhaseTimings) {
        // Convert ms to seconds for histogram
        self.phase_peer_selection
            .observe(timings.peer_selection_ms / 1000.0);
        self.phase_key_share.observe(timings.key_share_ms / 1000.0);
        self.phase_dag_compare
            .observe(timings.dag_compare_ms / 1000.0);
        self.phase_data_transfer
            .observe(timings.data_transfer_ms / 1000.0);
        self.phase_timeout_wait
            .observe(timings.timeout_wait_ms / 1000.0);
        self.phase_merge.observe(timings.merge_ms / 1000.0);

        // Record counters
        self.merge_operations.inc_by(timings.merge_count);
        self.hash_comparisons.inc_by(timings.hash_compare_count);
        self.bytes_received.inc_by(timings.bytes_received);
        self.bytes_sent.inc_by(timings.bytes_sent);
    }
}

/// RAII guard for tracking sync operation timing and outcome.
///
/// When dropped, records the duration and outcome (failure if not explicitly marked success).
pub struct SyncTimingGuard {
    metrics: SyncMetrics,
    protocol: String,
    start: Instant,
    completed: bool,
}

impl SyncTimingGuard {
    /// Mark the sync as successful and record metrics.
    pub fn success(mut self) {
        self.completed = true;
        let duration = self.start.elapsed().as_secs_f64();
        self.metrics
            .record_completion(&self.protocol, duration, true);
    }

    /// Mark the sync as failed and record metrics.
    pub fn failure(mut self) {
        self.completed = true;
        let duration = self.start.elapsed().as_secs_f64();
        self.metrics
            .record_completion(&self.protocol, duration, false);
    }

    /// Get elapsed time in milliseconds (for logging).
    pub fn elapsed_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }
}

impl Drop for SyncTimingGuard {
    fn drop(&mut self) {
        // If not explicitly completed, treat as failure
        if !self.completed {
            let duration = self.start.elapsed().as_secs_f64();
            self.metrics
                .record_completion(&self.protocol, duration, false);
        }
    }
}

/// Shared sync metrics handle (Arc-wrapped for cloning).
pub type SharedSyncMetrics = Arc<SyncMetrics>;

/// Create shared sync metrics.
pub fn create_sync_metrics(registry: &mut Registry) -> SharedSyncMetrics {
    Arc::new(SyncMetrics::new(registry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_timing_guard_success() {
        let mut registry = Registry::default();
        let metrics = SyncMetrics::new(&mut registry);

        {
            let guard = metrics.start_sync("snapshot");
            std::thread::sleep(std::time::Duration::from_millis(10));
            guard.success();
        }

        // Note: We can't easily read prometheus metric values in tests,
        // but we verify it doesn't panic and the flow works.
    }

    #[test]
    fn test_sync_timing_guard_failure() {
        let mut registry = Registry::default();
        let metrics = SyncMetrics::new(&mut registry);

        {
            let guard = metrics.start_sync("delta");
            guard.failure();
        }
    }

    #[test]
    fn test_sync_timing_guard_drop_without_complete() {
        let mut registry = Registry::default();
        let metrics = SyncMetrics::new(&mut registry);

        {
            let _guard = metrics.start_sync("dag_catchup");
            // Dropped without calling success() or failure()
            // Should be recorded as failure
        }
    }

    #[test]
    fn test_record_counters() {
        let mut registry = Registry::default();
        let metrics = SyncMetrics::new(&mut registry);

        metrics.record_snapshot_records(100);
        metrics.record_bytes_received(1024);
        metrics.record_bytes_sent(512);
        metrics.record_deltas_fetched(5);
        metrics.record_deltas_applied(5);
    }
}
