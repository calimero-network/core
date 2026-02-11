//! Message routing and delivery scheduling.
//!
//! See spec §2 - SimNetwork and §12 - Partition Modeling.

use super::faults::FaultConfig;
use super::partition::PartitionManager;
use crate::sync_sim::actions::{OutgoingMessage, SyncMessage};
use crate::sync_sim::metrics::EffectMetrics;
use crate::sync_sim::runtime::{EventQueue, SimDuration, SimRng, SimTime};
use crate::sync_sim::types::{MessageId, NodeId};

/// Event types for the simulation.
#[derive(Debug, Clone)]
pub enum SimEvent {
    /// Deliver a message to a node.
    DeliverMessage {
        from: NodeId,
        to: NodeId,
        msg: SyncMessage,
        msg_id: MessageId,
    },
    /// Fire a timer.
    TimerFired { node: NodeId, timer_id: u64 },
    /// Node crash event.
    NodeCrash { node: NodeId },
    /// Node restart event.
    NodeRestart { node: NodeId },
    /// Partition start.
    PartitionStart { groups: Vec<Vec<NodeId>> },
    /// Partition end.
    PartitionEnd { groups: Vec<Vec<NodeId>> },
}

/// Message in flight with delivery metadata.
#[derive(Debug)]
pub struct InFlightMessage {
    /// Source node.
    pub from: NodeId,
    /// Destination node.
    pub to: NodeId,
    /// The message.
    pub msg: SyncMessage,
    /// Message ID for deduplication.
    pub msg_id: MessageId,
    /// Scheduled delivery time.
    pub delivery_time: SimTime,
}

/// Network router for message delivery.
pub struct NetworkRouter {
    /// Fault configuration.
    fault_config: FaultConfig,
    /// Partition manager.
    partitions: PartitionManager,
    /// RNG for fault injection.
    rng: SimRng,
    /// Messages currently in flight (for counting).
    in_flight_count: usize,
    /// Metrics.
    pub metrics: NetworkMetrics,
}

/// Network metrics.
#[derive(Debug, Default, Clone)]
pub struct NetworkMetrics {
    /// Total messages sent.
    pub messages_sent: u64,
    /// Messages dropped due to loss.
    pub messages_dropped_loss: u64,
    /// Messages dropped due to partition.
    pub messages_dropped_partition: u64,
    /// Messages reordered.
    pub messages_reordered: u64,
    /// Messages duplicated.
    pub messages_duplicated: u64,
    /// Total bytes sent.
    pub bytes_sent: u64,
}

impl NetworkRouter {
    /// Create a new router with default config.
    pub fn new(seed: u64) -> Self {
        Self {
            fault_config: FaultConfig::default(),
            partitions: PartitionManager::new(),
            rng: SimRng::new(seed),
            in_flight_count: 0,
            metrics: NetworkMetrics::default(),
        }
    }

    /// Create with custom fault config.
    pub fn with_faults(seed: u64, config: FaultConfig) -> Self {
        Self {
            fault_config: config,
            partitions: PartitionManager::new(),
            rng: SimRng::new(seed),
            in_flight_count: 0,
            metrics: NetworkMetrics::default(),
        }
    }

    /// Set fault configuration.
    pub fn set_fault_config(&mut self, config: FaultConfig) {
        self.fault_config = config;
    }

    /// Get fault configuration.
    pub fn fault_config(&self) -> &FaultConfig {
        &self.fault_config
    }

    /// Get partition manager.
    pub fn partitions(&self) -> &PartitionManager {
        &self.partitions
    }

    /// Get mutable partition manager.
    pub fn partitions_mut(&mut self) -> &mut PartitionManager {
        &mut self.partitions
    }

    /// Get number of messages in flight.
    pub fn in_flight_count(&self) -> usize {
        self.in_flight_count
    }

    /// Increment in-flight count (for manually injected messages).
    pub fn increment_in_flight(&mut self) {
        self.in_flight_count += 1;
    }

    /// Route a message, potentially applying faults.
    ///
    /// Returns events to schedule (may be empty if message lost, or multiple if duplicated).
    /// Also updates the provided `effect_metrics` to reflect any network faults applied.
    pub fn route_message(
        &mut self,
        now: SimTime,
        msg: OutgoingMessage,
        from: &NodeId,
        queue: &mut EventQueue<SimEvent>,
        effect_metrics: &mut EffectMetrics,
    ) {
        self.metrics.messages_sent += 1;
        // Estimate message size (stack size; heap allocations not fully captured)
        let msg_size = std::mem::size_of_val(&msg.msg) as u64;
        self.metrics.bytes_sent += msg_size;

        // Check for partition at send time
        // Note: We also check at delivery time (spec §12.2)
        // Checking here is optional but can short-circuit

        // Check for message loss
        if self
            .rng
            .bool_with_probability(self.fault_config.message_loss_rate)
        {
            self.metrics.messages_dropped_loss += 1;
            effect_metrics.record_drop();
            return;
        }

        // Calculate delivery time with latency and jitter
        let base_latency = SimDuration::from_millis(self.fault_config.base_latency_ms);
        let jitter = SimDuration::from_millis(self.fault_config.latency_jitter_ms);
        let mut delivery_delay = self.rng.duration_with_jitter(base_latency, jitter);

        // Apply reorder: add random delay within reorder window
        // This causes messages to potentially arrive out of order
        if self.fault_config.reorder_window_ms > 0 {
            // Use saturating arithmetic to prevent overflow with large reorder_window_ms values
            let reorder_window_micros =
                (self.fault_config.reorder_window_ms as usize).saturating_mul(1000);
            // Ensure we have at least 1 to avoid issues with gen_range_usize(0)
            let reorder_delay_micros = if reorder_window_micros > 0 {
                self.rng.gen_range_usize(reorder_window_micros)
            } else {
                0
            };
            delivery_delay = delivery_delay + SimDuration::from_micros(reorder_delay_micros as u64);
            self.metrics.messages_reordered += 1;
            effect_metrics.record_reorder();
        }

        let delivery_time = now + delivery_delay;

        // Create the delivery event
        let event = SimEvent::DeliverMessage {
            from: from.clone(),
            to: msg.to.clone(),
            msg: msg.msg.clone(),
            msg_id: msg.msg_id.clone(),
        };

        // Schedule delivery
        queue.schedule(delivery_time, event);
        self.in_flight_count += 1;

        // Check for duplication
        if self
            .rng
            .bool_with_probability(self.fault_config.duplicate_rate)
        {
            self.metrics.messages_duplicated += 1;
            effect_metrics.record_duplicate();

            // Schedule duplicate with additional delay
            let dup_delay = self.rng.duration_with_jitter(base_latency, jitter);
            let dup_time = now + delivery_delay + dup_delay;

            let dup_event = SimEvent::DeliverMessage {
                from: from.clone(),
                to: msg.to,
                msg: msg.msg,
                msg_id: msg.msg_id,
            };

            queue.schedule(dup_time, dup_event);
            self.in_flight_count += 1;
        }
    }

    /// Check if delivery should proceed (partition check at delivery time).
    ///
    /// Returns true if message should be delivered, false if dropped.
    pub fn should_deliver(&mut self, from: &NodeId, to: &NodeId, now: SimTime) -> bool {
        self.in_flight_count = self.in_flight_count.saturating_sub(1);

        if self.partitions.is_partitioned(from, to, now) {
            self.metrics.messages_dropped_partition += 1;
            return false;
        }

        true
    }

    /// Reset metrics.
    pub fn reset_metrics(&mut self) {
        self.metrics = NetworkMetrics::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_sim::actions::SyncMessage;

    #[test]
    fn test_router_basic_delivery() {
        let mut router = NetworkRouter::new(42);
        let mut queue = EventQueue::new();
        let mut effects = EffectMetrics::default();
        let now = SimTime::ZERO;

        let msg = OutgoingMessage {
            to: NodeId::new("bob"),
            msg: SyncMessage::SyncComplete { success: true },
            msg_id: MessageId::new("alice", 1, 1),
        };

        router.route_message(now, msg, &NodeId::new("alice"), &mut queue, &mut effects);

        assert_eq!(router.metrics.messages_sent, 1);
        assert!(!queue.is_empty());
    }

    #[test]
    fn test_router_message_loss() {
        let mut router = NetworkRouter::with_faults(
            42,
            FaultConfig {
                message_loss_rate: 1.0, // 100% loss
                ..Default::default()
            },
        );
        let mut queue = EventQueue::new();
        let mut effects = EffectMetrics::default();
        let now = SimTime::ZERO;

        let msg = OutgoingMessage {
            to: NodeId::new("bob"),
            msg: SyncMessage::SyncComplete { success: true },
            msg_id: MessageId::new("alice", 1, 1),
        };

        router.route_message(now, msg, &NodeId::new("alice"), &mut queue, &mut effects);

        assert_eq!(router.metrics.messages_sent, 1);
        assert_eq!(router.metrics.messages_dropped_loss, 1);
        assert_eq!(effects.messages_dropped, 1);
        assert!(queue.is_empty()); // Message was lost
    }

    #[test]
    fn test_router_duplication() {
        let mut router = NetworkRouter::with_faults(
            42,
            FaultConfig {
                duplicate_rate: 1.0, // 100% duplication
                ..Default::default()
            },
        );
        let mut queue = EventQueue::new();
        let mut effects = EffectMetrics::default();
        let now = SimTime::ZERO;

        let msg = OutgoingMessage {
            to: NodeId::new("bob"),
            msg: SyncMessage::SyncComplete { success: true },
            msg_id: MessageId::new("alice", 1, 1),
        };

        router.route_message(now, msg, &NodeId::new("alice"), &mut queue, &mut effects);

        assert_eq!(router.metrics.messages_sent, 1);
        assert_eq!(router.metrics.messages_duplicated, 1);
        assert_eq!(effects.messages_duplicated, 1);
        assert_eq!(queue.len(), 2); // Original + duplicate
    }

    #[test]
    fn test_router_latency() {
        let mut router = NetworkRouter::with_faults(
            42,
            FaultConfig {
                base_latency_ms: 100,
                latency_jitter_ms: 10,
                ..Default::default()
            },
        );
        let mut queue = EventQueue::new();
        let mut effects = EffectMetrics::default();
        let now = SimTime::from_millis(1000);

        let msg = OutgoingMessage {
            to: NodeId::new("bob"),
            msg: SyncMessage::SyncComplete { success: true },
            msg_id: MessageId::new("alice", 1, 1),
        };

        router.route_message(now, msg, &NodeId::new("alice"), &mut queue, &mut effects);

        let (delivery_time, _, _) = queue.pop().unwrap();
        let delay = delivery_time - now;

        // Should be within base ± jitter
        assert!(delay.as_millis() >= 90);
        assert!(delay.as_millis() <= 110);
    }

    #[test]
    fn test_router_reorder() {
        let mut router = NetworkRouter::with_faults(
            42,
            FaultConfig {
                base_latency_ms: 10,
                latency_jitter_ms: 0,
                reorder_window_ms: 50, // 50ms reorder window
                ..Default::default()
            },
        );
        let mut queue = EventQueue::new();
        let mut effects = EffectMetrics::default();
        let now = SimTime::ZERO;

        // Send multiple messages
        for seq in 0..10 {
            let msg = OutgoingMessage {
                to: NodeId::new("bob"),
                msg: SyncMessage::SyncComplete { success: true },
                msg_id: MessageId::new("alice", 1, seq),
            };
            router.route_message(now, msg, &NodeId::new("alice"), &mut queue, &mut effects);
        }

        // Reorder metric should be counted
        assert_eq!(router.metrics.messages_reordered, 10);
        assert_eq!(effects.messages_reordered, 10);

        // Collect delivery times
        let mut delivery_times = Vec::new();
        while let Some((time, _, _)) = queue.pop() {
            delivery_times.push(time);
        }

        // With reorder, delay should be base + random(0..reorder_window)
        // So delays should be between 10ms and 60ms
        for time in &delivery_times {
            let delay_ms = time.as_millis();
            assert!(delay_ms >= 10, "delay {} should be >= 10", delay_ms);
            assert!(delay_ms <= 60, "delay {} should be <= 60", delay_ms);
        }
    }
}
