//! Main simulation runtime.
//!
//! See spec §2 - Architecture Overview.

use std::collections::HashMap;

use crate::sync_sim::actions::{SyncActions, SyncMessage, TimerOp};
use crate::sync_sim::convergence::{
    check_convergence, is_deadlocked, ConvergenceInput, ConvergenceResult, NodeConvergenceState,
};
use crate::sync_sim::metrics::SimMetrics;
use crate::sync_sim::network::{FaultConfig, NetworkRouter, SimEvent};
use crate::sync_sim::node::SimNode;
use crate::sync_sim::runtime::{EventQueue, SimClock, SimDuration, SimRng, SimTime};
use crate::sync_sim::types::NodeId;

/// Simulation configuration.
#[derive(Debug, Clone)]
pub struct SimConfig {
    /// Random seed.
    pub seed: u64,
    /// Maximum simulation time before stopping.
    pub max_time: SimTime,
    /// Maximum events before stopping.
    pub max_events: u64,
    /// Process all pending messages per node per tick (vs round-robin).
    pub drain_inbox_per_tick: bool,
    /// Fault configuration.
    pub fault_config: FaultConfig,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            seed: 0,
            max_time: SimTime::from_secs(60),
            max_events: 1_000_000,
            drain_inbox_per_tick: false,
            fault_config: FaultConfig::default(),
        }
    }
}

impl SimConfig {
    /// Create with seed.
    pub fn with_seed(seed: u64) -> Self {
        Self {
            seed,
            ..Default::default()
        }
    }

    /// Builder: set max time.
    pub fn max_time(mut self, time: SimTime) -> Self {
        self.max_time = time;
        self
    }

    /// Builder: set max events.
    pub fn max_events(mut self, events: u64) -> Self {
        self.max_events = events;
        self
    }

    /// Builder: enable drain inbox mode.
    pub fn drain_inbox(mut self) -> Self {
        self.drain_inbox_per_tick = true;
        self
    }

    /// Builder: set fault config.
    pub fn with_faults(mut self, config: FaultConfig) -> Self {
        self.fault_config = config;
        self
    }
}

/// Stop condition for simulation.
#[derive(Debug, Clone, PartialEq)]
pub enum StopCondition {
    /// Reached maximum time.
    MaxTime,
    /// Reached maximum events.
    MaxEvents,
    /// System converged.
    Converged,
    /// System deadlocked.
    Deadlock,
    /// System quiesced in a diverged state (not converged, not deadlocked, no pending events).
    Diverged,
    /// Manually stopped.
    Manual,
}

/// Main simulation runtime.
pub struct SimRuntime {
    /// Configuration.
    config: SimConfig,
    /// Logical clock.
    clock: SimClock,
    /// Event queue.
    queue: EventQueue<SimEvent>,
    /// Network router.
    network: NetworkRouter,
    /// RNG.
    rng: SimRng,
    /// Nodes by ID.
    nodes: HashMap<NodeId, SimNode>,
    /// Node processing order (sorted by ID).
    node_order: Vec<NodeId>,
    /// Collected metrics.
    metrics: SimMetrics,
    /// Events processed count.
    events_processed: u64,
    /// Total messages sent.
    messages_sent: u64,
}

impl SimRuntime {
    /// Create a new runtime with seed.
    pub fn new(seed: u64) -> Self {
        let config = SimConfig::with_seed(seed);
        Self::with_config(config)
    }

    /// Create with configuration.
    pub fn with_config(config: SimConfig) -> Self {
        // Note: drain_inbox_per_tick is not yet implemented.
        // Assert in debug builds to catch unintended usage.
        debug_assert!(
            !config.drain_inbox_per_tick,
            "drain_inbox_per_tick is not yet implemented; messages are processed one at a time"
        );

        let rng = SimRng::new(config.seed);
        // Use wrapping_add to avoid overflow panic when seed is u64::MAX
        let network =
            NetworkRouter::with_faults(config.seed.wrapping_add(1), config.fault_config.clone());

        Self {
            config,
            clock: SimClock::new(),
            queue: EventQueue::new(),
            network,
            rng,
            nodes: HashMap::new(),
            node_order: Vec::new(),
            metrics: SimMetrics::new(),
            events_processed: 0,
            messages_sent: 0,
        }
    }

    // =========================================================================
    // Node Management
    // =========================================================================

    /// Add a node to the simulation.
    ///
    /// If a node with the same ID already exists, it will be replaced
    /// but not duplicated in node_order.
    pub fn add_node(&mut self, id: impl Into<NodeId>) -> NodeId {
        let id = id.into();
        let is_new = !self.nodes.contains_key(&id);
        let node = SimNode::new(id.clone());
        self.nodes.insert(id.clone(), node);
        if is_new {
            self.node_order.push(id.clone());
            self.node_order.sort();
        }
        id
    }

    /// Add multiple nodes.
    pub fn add_nodes(&mut self, ids: impl IntoIterator<Item = impl Into<NodeId>>) -> Vec<NodeId> {
        ids.into_iter().map(|id| self.add_node(id)).collect()
    }

    /// Get a node by ID.
    pub fn node(&self, id: &NodeId) -> Option<&SimNode> {
        self.nodes.get(id)
    }

    /// Get a mutable node by ID.
    pub fn node_mut(&mut self, id: &NodeId) -> Option<&mut SimNode> {
        self.nodes.get_mut(id)
    }

    /// Get all node IDs.
    pub fn node_ids(&self) -> Vec<NodeId> {
        self.node_order.clone()
    }

    /// Get number of nodes.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Add a pre-configured node.
    ///
    /// If a node with the same ID already exists, it will be replaced
    /// but not duplicated in node_order.
    pub fn add_existing_node(&mut self, node: SimNode) -> NodeId {
        let id = node.id().clone();
        let is_new = !self.nodes.contains_key(&id);
        self.nodes.insert(id.clone(), node);
        if is_new {
            self.node_order.push(id.clone());
            self.node_order.sort();
        }
        id
    }

    // =========================================================================
    // Simulation Control
    // =========================================================================

    /// Get current simulation time.
    pub fn now(&self) -> SimTime {
        self.clock.now()
    }

    /// Get metrics.
    pub fn metrics(&self) -> &SimMetrics {
        &self.metrics
    }

    /// Get mutable metrics.
    pub fn metrics_mut(&mut self) -> &mut SimMetrics {
        &mut self.metrics
    }

    /// Get events processed count.
    pub fn events_processed(&self) -> u64 {
        self.events_processed
    }

    /// Get network router.
    pub fn network(&self) -> &NetworkRouter {
        &self.network
    }

    /// Get mutable network router.
    pub fn network_mut(&mut self) -> &mut NetworkRouter {
        &mut self.network
    }

    /// Get RNG.
    pub fn rng(&mut self) -> &mut SimRng {
        &mut self.rng
    }

    /// Check convergence.
    pub fn check_convergence(&mut self) -> ConvergenceResult {
        let input = self.build_convergence_input();
        check_convergence(&input)
    }

    /// Check if system is deadlocked.
    pub fn is_deadlocked(&mut self) -> bool {
        let input = self.build_convergence_input();
        is_deadlocked(&input, self.queue.is_empty())
    }

    fn build_convergence_input(&mut self) -> ConvergenceInput {
        let nodes: Vec<_> = self
            .node_order
            .iter()
            .map(|id| {
                let node = self.nodes.get_mut(id).unwrap();
                NodeConvergenceState {
                    id: id.clone(),
                    sync_active: node.sync_state.is_active(),
                    buffer_size: node.buffer_size(),
                    sync_timer_count: node.sync_timer_count(),
                    digest: node.state_digest(),
                }
            })
            .collect();

        ConvergenceInput {
            in_flight_messages: self.network.in_flight_count(),
            nodes,
        }
    }

    // =========================================================================
    // Event Scheduling
    // =========================================================================

    /// Schedule an event.
    ///
    /// # Panics
    /// Panics if `time` is before the current simulation time, as this would
    /// cause a clock advancement error when the event is processed.
    pub fn schedule(&mut self, time: SimTime, event: SimEvent) {
        assert!(
            time >= self.clock.now(),
            "Cannot schedule event in the past: event time {} < current time {}",
            time,
            self.clock.now()
        );
        self.queue.schedule(time, event);
    }

    /// Schedule event after delay.
    pub fn schedule_after(&mut self, delay: SimDuration, event: SimEvent) {
        let time = self.clock.now() + delay;
        self.queue.schedule(time, event);
    }

    // =========================================================================
    // Running
    // =========================================================================

    /// Run simulation until a stop condition is met.
    pub fn run(&mut self) -> StopCondition {
        loop {
            // Check stop conditions
            if self.clock.now() >= self.config.max_time {
                return StopCondition::MaxTime;
            }

            if self.events_processed >= self.config.max_events {
                return StopCondition::MaxEvents;
            }

            // If there are events pending, process them before checking convergence.
            // This ensures scheduled events are executed before declaring convergence.
            if !self.queue.is_empty() {
                // Check if the next event would exceed max_time before processing it.
                // This prevents executing events beyond the configured time bound.
                if let Some(next_time) = self.queue.peek_time() {
                    if next_time >= self.config.max_time {
                        return StopCondition::MaxTime;
                    }
                }
                self.step();
                continue;
            }

            // Queue is empty - now check convergence
            if self.check_convergence().is_converged() {
                self.metrics.convergence.mark_converged(
                    self.clock.now(),
                    self.messages_sent,
                    self.events_processed,
                );
                return StopCondition::Converged;
            }

            // Check deadlock (queue empty but not converged)
            if self.is_deadlocked() {
                self.metrics.convergence.mark_failed("deadlock".to_string());
                return StopCondition::Deadlock;
            }

            // Queue empty, not converged, not deadlocked - system is diverged
            self.metrics.convergence.mark_failed("diverged".to_string());
            return StopCondition::Diverged;
        }
    }

    /// Run until convergence or timeout.
    pub fn run_until_converged(&mut self) -> bool {
        let condition = self.run();
        matches!(condition, StopCondition::Converged)
    }

    /// Run until a predicate is true.
    pub fn run_until<F>(&mut self, mut predicate: F) -> StopCondition
    where
        F: FnMut(&Self) -> bool,
    {
        loop {
            if predicate(self) {
                return StopCondition::Manual;
            }

            if self.clock.now() >= self.config.max_time {
                return StopCondition::MaxTime;
            }

            if self.events_processed >= self.config.max_events {
                return StopCondition::MaxEvents;
            }

            if self.queue.is_empty() {
                // Queue empty - check if system is diverged
                if self.check_convergence().is_diverged() {
                    self.metrics.convergence.mark_failed("diverged".to_string());
                    return StopCondition::Diverged;
                }
                return StopCondition::Manual;
            }

            // Check if the next event would exceed max_time before processing it.
            if let Some(next_time) = self.queue.peek_time() {
                if next_time >= self.config.max_time {
                    return StopCondition::MaxTime;
                }
            }

            self.step();
        }
    }

    /// Process a single event.
    pub fn step(&mut self) -> bool {
        let Some((time, _seq, event)) = self.queue.pop() else {
            return false;
        };

        // For timer events, check if timer is still valid BEFORE advancing clock.
        // Cancelled or rescheduled timers should not advance simulation time, but they
        // still count toward the event budget since they were dequeued and processed.
        if let SimEvent::TimerFired { node, timer_id } = &event {
            let timer_id_typed = crate::sync_sim::types::TimerId::new(*timer_id);
            if let Some(sim_node) = self.nodes.get(node) {
                // Skip if node is crashed
                if sim_node.is_crashed {
                    self.events_processed += 1;
                    return true;
                }
                // Skip if timer was cancelled or rescheduled
                match sim_node.get_timer(timer_id_typed) {
                    None => {
                        // Timer was cancelled
                        self.events_processed += 1;
                        return true;
                    }
                    Some(entry) if entry.fire_time != time => {
                        // Stale event from reschedule
                        self.events_processed += 1;
                        return true;
                    }
                    Some(_) => {} // Valid timer, proceed
                }
            }
        }

        // Advance clock
        self.clock.advance_to(time);
        self.events_processed += 1;

        // Process event
        match event {
            SimEvent::DeliverMessage {
                from,
                to,
                msg,
                msg_id,
            } => {
                // Check partition at delivery time
                if !self.network.should_deliver(&from, &to, time) {
                    return true;
                }

                // Get node and check duplicate
                let Some(node) = self.nodes.get_mut(&to) else {
                    return true;
                };

                // Crashed nodes cannot receive messages
                if node.is_crashed {
                    return true;
                }

                if node.is_duplicate(&msg_id) {
                    return true;
                }

                node.mark_processed(msg_id);

                // Process message and get actions
                // Note: handle_message currently doesn't need &mut self, so we can pass the node
                let actions = Self::handle_message_static(&from, msg);

                // Apply actions
                self.apply_actions(&to, actions);
            }

            SimEvent::TimerFired { node, timer_id } => {
                let node_id = node.clone();
                let timer_id_typed = crate::sync_sim::types::TimerId::new(timer_id);

                let Some(sim_node) = self.nodes.get_mut(&node) else {
                    return true;
                };

                // Crashed nodes cannot process timer events
                if sim_node.is_crashed {
                    return true;
                }

                // Check timer still exists and fire_time matches (handles rescheduled timers)
                // If the timer was rescheduled, the stored fire_time won't match this event's time
                let timer = sim_node.get_timer(timer_id_typed);
                match timer {
                    None => return true,                                   // Timer was cancelled
                    Some(entry) if entry.fire_time != time => return true, // Stale event from before reschedule
                    Some(_) => {}                                          // Valid timer fire
                }

                // Remove the fired timer from the node
                sim_node.cancel_timer(timer_id_typed);

                // Process timeout
                let actions = Self::handle_timeout_static(timer_id);

                // Apply actions
                self.apply_actions(&node_id, actions);
            }

            SimEvent::NodeCrash { node } => {
                if let Some(n) = self.nodes.get_mut(&node) {
                    n.crash();
                    self.metrics.effects.record_crash();
                }
            }

            SimEvent::NodeRestart { node } => {
                if let Some(n) = self.nodes.get_mut(&node) {
                    // Only restart if node is actually crashed to avoid spurious session increments
                    if n.is_crashed {
                        n.restart();
                        self.metrics.effects.record_restart();
                    }
                }
            }

            SimEvent::PartitionStart { groups } => {
                use crate::sync_sim::network::PartitionSpec;
                self.network.partitions_mut().add_partition(
                    PartitionSpec::Bidirectional { groups },
                    time,
                    None,
                );
                self.metrics.effects.record_partition();
            }

            SimEvent::PartitionEnd { groups } => {
                use crate::sync_sim::network::PartitionSpec;
                // Normalize groups for comparison (sort each group and sort groups by first element)
                // This ensures partition matching is independent of group/node ordering
                let normalize = |groups: &Vec<Vec<NodeId>>| -> Vec<Vec<NodeId>> {
                    let mut normalized: Vec<Vec<NodeId>> = groups
                        .iter()
                        .map(|g| {
                            let mut sorted = g.clone();
                            sorted.sort();
                            sorted
                        })
                        .collect();
                    normalized.sort_by(|a, b| a.first().cmp(&b.first()));
                    normalized
                };
                let target = normalize(&groups);
                self.network.partitions_mut().remove_partitions(|spec| {
                    matches!(spec, PartitionSpec::Bidirectional { groups: g } if normalize(g) == target)
                });
            }
        }

        true
    }

    /// Handle incoming message (placeholder - protocol-specific).
    fn handle_message_static(_from: &NodeId, _msg: SyncMessage) -> SyncActions {
        // Protocol-specific handling will be implemented in later phases
        // For now, return empty actions
        SyncActions::new()
    }

    /// Handle timeout (placeholder - protocol-specific).
    fn handle_timeout_static(_timer_id: u64) -> SyncActions {
        // Protocol-specific handling will be implemented in later phases
        SyncActions::new()
    }

    /// Apply actions from a node.
    fn apply_actions(&mut self, node_id: &NodeId, actions: SyncActions) {
        // Apply storage operations
        if let Some(node) = self.nodes.get_mut(node_id) {
            for op in actions.storage_ops {
                node.apply_storage_op(op);
                self.metrics.work.record_write();
            }

            // Apply timer operations
            for timer_op in actions.timer_ops {
                match timer_op {
                    TimerOp::Set { id, delay, kind } => {
                        let fire_time = self.clock.now() + delay;
                        node.set_timer(id, fire_time, kind);

                        // Schedule timer event
                        self.queue.schedule(
                            fire_time,
                            SimEvent::TimerFired {
                                node: node_id.clone(),
                                timer_id: id.0,
                            },
                        );
                    }
                    TimerOp::Cancel { id } => {
                        node.cancel_timer(id);
                        // Note: We don't remove from queue; it will be ignored when fired
                    }
                }
            }
        }

        // Route messages
        for msg in actions.messages {
            self.messages_sent += 1;
            self.metrics.protocol.record_message(0); // TODO: actual size

            // Clone node_id before borrow
            let from = node_id.clone();
            let now = self.clock.now();
            self.network
                .route_message(now, msg, &from, &mut self.queue, &mut self.metrics.effects);
        }
    }

    // =========================================================================
    // Test Helpers
    // =========================================================================

    /// Inject a message directly into the queue.
    ///
    /// Note: This bypasses fault injection (loss, reorder, etc.) but properly
    /// accounts for the message in convergence tracking.
    ///
    /// Returns None if the sender node doesn't exist.
    pub fn inject_message(
        &mut self,
        from: NodeId,
        to: NodeId,
        msg: SyncMessage,
        delay: SimDuration,
    ) -> Option<()> {
        let msg_id = {
            let node = self.nodes.get_mut(&from)?;
            node.next_message_id()
        };

        let delivery_time = self.clock.now() + delay;
        self.queue.schedule(
            delivery_time,
            SimEvent::DeliverMessage {
                from,
                to,
                msg,
                msg_id,
            },
        );

        // Track in-flight message for convergence checking
        self.network.increment_in_flight();
        Some(())
    }

    /// Crash a node after delay.
    pub fn schedule_crash(&mut self, node: NodeId, delay: SimDuration) {
        let time = self.clock.now() + delay;
        self.queue.schedule(time, SimEvent::NodeCrash { node });
    }

    /// Restart a node after delay.
    pub fn schedule_restart(&mut self, node: NodeId, delay: SimDuration) {
        let time = self.clock.now() + delay;
        self.queue.schedule(time, SimEvent::NodeRestart { node });
    }

    /// Create a partition.
    pub fn schedule_partition(&mut self, groups: Vec<Vec<NodeId>>, delay: SimDuration) {
        let time = self.clock.now() + delay;
        self.queue
            .schedule(time, SimEvent::PartitionStart { groups });
    }

    /// Heal a partition.
    pub fn schedule_heal(&mut self, groups: Vec<Vec<NodeId>>, delay: SimDuration) {
        let time = self.clock.now() + delay;
        self.queue.schedule(time, SimEvent::PartitionEnd { groups });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_sim::types::EntityId;
    use calimero_primitives::crdt::CrdtType;

    #[test]
    fn test_runtime_creation() {
        let rt = SimRuntime::new(42);
        assert_eq!(rt.now(), SimTime::ZERO);
        assert_eq!(rt.node_count(), 0);
    }

    #[test]
    fn test_add_nodes() {
        let mut rt = SimRuntime::new(42);

        let a = rt.add_node("alice");
        let b = rt.add_node("bob");

        assert_eq!(rt.node_count(), 2);
        assert!(rt.node(&a).is_some());
        assert!(rt.node(&b).is_some());

        // Node order should be sorted
        assert_eq!(rt.node_ids(), vec![a, b]);
    }

    #[test]
    fn test_convergence_empty() {
        let mut rt = SimRuntime::new(42);

        // Empty system is converged
        assert!(rt.check_convergence().is_converged());
    }

    #[test]
    fn test_convergence_same_state() {
        let mut rt = SimRuntime::new(42);

        let a = rt.add_node("alice");
        let b = rt.add_node("bob");

        // Same empty state
        assert!(rt.check_convergence().is_converged());

        // Add same entity to both
        let id = EntityId::from_u64(1);
        rt.node_mut(&a)
            .unwrap()
            .insert_entity(id, vec![1, 2, 3], CrdtType::LwwRegister);
        rt.node_mut(&b)
            .unwrap()
            .insert_entity(id, vec![1, 2, 3], CrdtType::LwwRegister);

        assert!(rt.check_convergence().is_converged());
    }

    #[test]
    fn test_convergence_different_state() {
        let mut rt = SimRuntime::new(42);

        let a = rt.add_node("alice");
        let b = rt.add_node("bob");

        // Different entities
        rt.node_mut(&a).unwrap().insert_entity(
            EntityId::from_u64(1),
            vec![1],
            CrdtType::LwwRegister,
        );
        rt.node_mut(&b).unwrap().insert_entity(
            EntityId::from_u64(2),
            vec![2],
            CrdtType::LwwRegister,
        );

        assert!(rt.check_convergence().is_diverged());
    }

    #[test]
    fn test_schedule_and_step() {
        let mut rt = SimRuntime::new(42);
        let _a = rt.add_node("alice");

        // Schedule crash
        rt.schedule_crash(NodeId::new("alice"), SimDuration::from_millis(100));

        // Step should process it
        assert!(rt.step());
        assert_eq!(rt.now(), SimTime::from_millis(100));
    }

    #[test]
    fn test_inject_message() {
        let mut rt = SimRuntime::new(42);
        let a = rt.add_node("alice");
        let b = rt.add_node("bob");

        rt.inject_message(
            a.clone(),
            b,
            SyncMessage::SyncComplete { success: true },
            SimDuration::from_millis(50),
        )
        .expect("sender node should exist");

        // Message should be queued
        assert!(!rt.queue.is_empty());

        // Step should process it
        assert!(rt.step());
        assert_eq!(rt.now(), SimTime::from_millis(50));
    }

    #[test]
    fn test_partition() {
        let mut rt = SimRuntime::new(42);
        let a = rt.add_node("alice");
        let b = rt.add_node("bob");

        // Schedule partition
        rt.schedule_partition(
            vec![vec![a.clone()], vec![b.clone()]],
            SimDuration::from_millis(0),
        );

        // Process partition event
        rt.step();

        // Network should be partitioned
        assert!(rt.network().partitions().has_partitions());
    }

    #[test]
    fn test_crash_restart() {
        let mut rt = SimRuntime::new(42);
        let a = rt.add_node("alice");

        // Add some state
        rt.node_mut(&a).unwrap().insert_entity(
            EntityId::from_u64(1),
            vec![1],
            CrdtType::LwwRegister,
        );

        // Schedule crash and restart
        rt.schedule_crash(a.clone(), SimDuration::from_millis(100));
        rt.schedule_restart(a.clone(), SimDuration::from_millis(200));

        // Process crash
        rt.step();
        let node = rt.node(&a).unwrap();
        assert!(node.sync_state.is_idle());
        assert_eq!(node.session, 0); // Not incremented until restart

        // Process restart
        rt.step();
        let node = rt.node(&a).unwrap();
        assert_eq!(node.session, 1);

        // Storage should be preserved
        assert_eq!(node.entity_count(), 1);
    }

    #[test]
    fn test_fired_timer_is_removed() {
        use crate::sync_sim::types::TimerKind;

        let mut rt = SimRuntime::new(42);
        let a = rt.add_node("alice");

        // Set a timer on the node
        let timer_id = {
            let node = rt.node_mut(&a).unwrap();
            let tid = node.next_timer_id();
            node.set_timer(tid, SimTime::from_millis(100), TimerKind::Sync);
            tid
        };

        // Schedule the timer event
        rt.schedule(
            SimTime::from_millis(100),
            SimEvent::TimerFired {
                node: a.clone(),
                timer_id: timer_id.0,
            },
        );

        // Before firing, timer should exist
        assert!(rt.node(&a).unwrap().get_timer(timer_id).is_some());
        assert_eq!(rt.node(&a).unwrap().sync_timer_count(), 1);

        // Process the timer event
        rt.step();

        // After firing, timer should be removed
        assert!(rt.node(&a).unwrap().get_timer(timer_id).is_none());
        assert_eq!(rt.node(&a).unwrap().sync_timer_count(), 0);
    }
}
