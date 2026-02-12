//! Simulation metrics collection.
//!
//! See spec §14 - Metrics.

use super::runtime::SimTime;

/// Combined metrics for simulation.
#[derive(Debug, Default, Clone)]
pub struct SimMetrics {
    /// Protocol cost metrics.
    pub protocol: ProtocolMetrics,
    /// Simulation effect metrics.
    pub effects: EffectMetrics,
    /// Work done metrics.
    pub work: WorkMetrics,
    /// Convergence metrics.
    pub convergence: ConvergenceMetrics,
}

impl SimMetrics {
    /// Create new empty metrics.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset all metrics.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Merge metrics from another instance.
    pub fn merge(&mut self, other: &SimMetrics) {
        self.protocol.merge(&other.protocol);
        self.effects.merge(&other.effects);
        self.work.merge(&other.work);
        // Convergence metrics are not merged (they're per-run)
    }
}

/// Protocol cost metrics (spec §14.1).
#[derive(Debug, Default, Clone)]
pub struct ProtocolMetrics {
    /// Protocol messages emitted.
    pub messages_sent: u64,
    /// Application payload bytes.
    pub payload_bytes: u64,
    /// Request-response pairs.
    pub round_trips: u64,
    /// Hash comparisons.
    pub entities_compared: u64,
    /// Entities sent.
    pub entities_transferred: u64,
    /// CRDT merges.
    pub merges_performed: u64,
}

impl ProtocolMetrics {
    /// Record a message sent.
    pub fn record_message(&mut self, payload_bytes: usize) {
        self.messages_sent += 1;
        self.payload_bytes += payload_bytes as u64;
    }

    /// Record a round trip.
    pub fn record_round_trip(&mut self) {
        self.round_trips += 1;
    }

    /// Record entity comparison.
    pub fn record_comparison(&mut self) {
        self.entities_compared += 1;
    }

    /// Record entity transfer.
    pub fn record_transfer(&mut self) {
        self.entities_transferred += 1;
    }

    /// Record CRDT merge.
    pub fn record_merge(&mut self) {
        self.merges_performed += 1;
    }

    /// Merge from another.
    pub fn merge(&mut self, other: &ProtocolMetrics) {
        self.messages_sent += other.messages_sent;
        self.payload_bytes += other.payload_bytes;
        self.round_trips += other.round_trips;
        self.entities_compared += other.entities_compared;
        self.entities_transferred += other.entities_transferred;
        self.merges_performed += other.merges_performed;
    }
}

/// Simulation effect metrics (spec §14.2).
#[derive(Debug, Default, Clone)]
pub struct EffectMetrics {
    /// Lost to faults/partitions.
    pub messages_dropped: u64,
    /// Delivered out of order.
    pub messages_reordered: u64,
    /// Delivered multiple times.
    pub messages_duplicated: u64,
    /// Protocol timeouts fired.
    pub timeouts_triggered: u64,
    /// Protocol-level retransmissions.
    pub retries_performed: u64,
    /// Node crashes.
    pub node_crashes: u64,
    /// Node restarts.
    pub node_restarts: u64,
    /// Partition events.
    pub partitions: u64,
    /// Delta buffer drops due to overflow (Invariant I6 violation risk).
    pub buffer_drops: u64,
}

impl EffectMetrics {
    /// Record message drop.
    pub fn record_drop(&mut self) {
        self.messages_dropped += 1;
    }

    /// Record reorder.
    pub fn record_reorder(&mut self) {
        self.messages_reordered += 1;
    }

    /// Record duplicate.
    pub fn record_duplicate(&mut self) {
        self.messages_duplicated += 1;
    }

    /// Record timeout.
    pub fn record_timeout(&mut self) {
        self.timeouts_triggered += 1;
    }

    /// Record retry.
    pub fn record_retry(&mut self) {
        self.retries_performed += 1;
    }

    /// Record crash.
    pub fn record_crash(&mut self) {
        self.node_crashes += 1;
    }

    /// Record restart.
    pub fn record_restart(&mut self) {
        self.node_restarts += 1;
    }

    /// Record partition.
    pub fn record_partition(&mut self) {
        self.partitions += 1;
    }

    /// Record buffer drop (Invariant I6 violation risk).
    pub fn record_buffer_drop(&mut self) {
        self.buffer_drops += 1;
    }

    /// Merge from another.
    pub fn merge(&mut self, other: &EffectMetrics) {
        self.messages_dropped += other.messages_dropped;
        self.messages_reordered += other.messages_reordered;
        self.messages_duplicated += other.messages_duplicated;
        self.timeouts_triggered += other.timeouts_triggered;
        self.retries_performed += other.retries_performed;
        self.node_crashes += other.node_crashes;
        self.node_restarts += other.node_restarts;
        self.partitions += other.partitions;
        self.buffer_drops += other.buffer_drops;
    }
}

/// Work done metrics (spec §14.3).
#[derive(Debug, Default, Clone)]
pub struct WorkMetrics {
    /// Digest/hash calculations.
    pub hash_computations: u64,
    /// DAG ancestry queries.
    pub dag_lookups: u64,
    /// Entity reads.
    pub storage_reads: u64,
    /// Entity writes.
    pub storage_writes: u64,
}

impl WorkMetrics {
    /// Record hash computation.
    pub fn record_hash(&mut self) {
        self.hash_computations += 1;
    }

    /// Record DAG lookup.
    pub fn record_dag_lookup(&mut self) {
        self.dag_lookups += 1;
    }

    /// Record storage read.
    pub fn record_read(&mut self) {
        self.storage_reads += 1;
    }

    /// Record storage write.
    pub fn record_write(&mut self) {
        self.storage_writes += 1;
    }

    /// Merge from another.
    pub fn merge(&mut self, other: &WorkMetrics) {
        self.hash_computations += other.hash_computations;
        self.dag_lookups += other.dag_lookups;
        self.storage_reads += other.storage_reads;
        self.storage_writes += other.storage_writes;
    }
}

/// Convergence metrics (spec §14.4).
#[derive(Debug, Default, Clone)]
pub struct ConvergenceMetrics {
    /// From start to convergence.
    pub time_to_converge: Option<SimTime>,
    /// Total messages until converged.
    pub messages_to_converge: u64,
    /// Total events processed.
    pub events_to_converge: u64,
    /// Whether convergence was achieved.
    pub converged: bool,
    /// Reason if not converged.
    pub failure_reason: Option<String>,
}

impl ConvergenceMetrics {
    /// Mark as converged.
    pub fn mark_converged(&mut self, time: SimTime, messages: u64, events: u64) {
        self.converged = true;
        self.time_to_converge = Some(time);
        self.messages_to_converge = messages;
        self.events_to_converge = events;
        self.failure_reason = None;
    }

    /// Mark as failed.
    pub fn mark_failed(&mut self, reason: String) {
        self.converged = false;
        self.failure_reason = Some(reason);
        // Clear any prior success timestamps to avoid inconsistent state
        self.time_to_converge = None;
        self.messages_to_converge = 0;
        self.events_to_converge = 0;
    }
}

/// Per-node metrics.
#[derive(Debug, Default, Clone)]
pub struct NodeMetrics {
    /// Messages sent by this node.
    pub messages_sent: u64,
    /// Messages received by this node.
    pub messages_received: u64,
    /// Bytes sent.
    pub bytes_sent: u64,
    /// Bytes received.
    pub bytes_received: u64,
    /// Storage operations.
    pub storage_ops: u64,
    /// Sync sessions initiated.
    pub syncs_initiated: u64,
    /// Sync sessions completed.
    pub syncs_completed: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_metrics() {
        let mut metrics = ProtocolMetrics::default();

        metrics.record_message(100);
        metrics.record_message(200);
        metrics.record_round_trip();
        metrics.record_comparison();
        metrics.record_transfer();
        metrics.record_merge();

        assert_eq!(metrics.messages_sent, 2);
        assert_eq!(metrics.payload_bytes, 300);
        assert_eq!(metrics.round_trips, 1);
        assert_eq!(metrics.entities_compared, 1);
        assert_eq!(metrics.entities_transferred, 1);
        assert_eq!(metrics.merges_performed, 1);
    }

    #[test]
    fn test_metrics_merge() {
        let mut m1 = SimMetrics::new();
        m1.protocol.messages_sent = 10;
        m1.effects.messages_dropped = 2;

        let mut m2 = SimMetrics::new();
        m2.protocol.messages_sent = 5;
        m2.effects.messages_dropped = 1;

        m1.merge(&m2);

        assert_eq!(m1.protocol.messages_sent, 15);
        assert_eq!(m1.effects.messages_dropped, 3);
    }

    #[test]
    fn test_convergence_metrics() {
        let mut metrics = ConvergenceMetrics::default();

        assert!(!metrics.converged);

        metrics.mark_converged(SimTime::from_millis(1000), 50, 100);

        assert!(metrics.converged);
        assert_eq!(metrics.time_to_converge, Some(SimTime::from_millis(1000)));
        assert_eq!(metrics.messages_to_converge, 50);
        assert_eq!(metrics.events_to_converge, 100);

        metrics.mark_failed("timeout".to_string());
        assert!(!metrics.converged);
        assert_eq!(metrics.failure_reason, Some("timeout".to_string()));
    }
}
