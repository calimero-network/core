//! Sync Protocol Simulation Test Entry Point
//!
//! This module provides the test harness for the sync protocol simulation framework.
//!
//! # Running Tests
//!
//! ```bash
//! cargo test -p calimero-node --test sync_sim
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use sync_sim::prelude::*;
//!
//! #[test]
//! fn test_basic_convergence() {
//!     let mut rt = SimRuntime::new(42);
//!     
//!     // Use a deterministic scenario
//!     let (a, b) = Scenario::force_none();
//!     rt.add_existing_node(a);
//!     rt.add_existing_node(b);
//!     
//!     // Already synced - should converge immediately
//!     assert!(rt.check_convergence().is_converged());
//! }
//! ```

#[path = "sync_sim/mod.rs"]
pub mod sync_sim;

// Re-export prelude for convenience
pub use sync_sim::prelude::*;

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_primitives::crdt::CrdtType;

    // =========================================================================
    // Basic Tests
    // =========================================================================

    #[test]
    fn test_empty_runtime() {
        let mut rt = SimRuntime::new(42);
        assert!(rt.check_convergence().is_converged());
    }

    #[test]
    fn test_single_node() {
        let mut rt = SimRuntime::new(42);
        let a = rt.add_node("alice");

        rt.node_mut(&a).unwrap().insert_entity(
            EntityId::from_u64(1),
            vec![1, 2, 3],
            CrdtType::LwwRegister,
        );

        assert!(rt.check_convergence().is_converged());
    }

    #[test]
    fn test_two_nodes_same_state() {
        let mut rt = SimRuntime::new(42);

        let (a, b) = Scenario::force_none();
        let _a_id = rt.add_existing_node(a);
        let _b_id = rt.add_existing_node(b);

        assert!(rt.check_convergence().is_converged());
    }

    #[test]
    fn test_two_nodes_different_state() {
        let mut rt = SimRuntime::new(42);

        let (a, b) = Scenario::force_hash_high_divergence();
        rt.add_existing_node(a);
        rt.add_existing_node(b);

        assert!(rt.check_convergence().is_diverged());
    }

    // =========================================================================
    // Scenario Tests
    // =========================================================================

    #[test]
    fn test_scenario_force_none() {
        let (mut a, mut b) = Scenario::force_none();
        assert_eq!(a.root_hash(), b.root_hash());
    }

    #[test]
    fn test_scenario_force_snapshot() {
        let (fresh, source) = Scenario::force_snapshot();
        assert!(!fresh.has_any_state());
        assert!(source.has_any_state());
    }

    #[test]
    fn test_scenario_partial_overlap() {
        let (a, b) = Scenario::partial_overlap();
        assert_eq!(a.entity_count(), 75);
        assert_eq!(b.entity_count(), 75);
    }

    #[test]
    fn test_scenario_both_initialized() {
        let (a, b) = Scenario::both_initialized();
        assert!(a.has_any_state());
        assert!(b.has_any_state());
    }

    // =========================================================================
    // Convergence Tests
    // =========================================================================

    #[test]
    fn test_convergence_pending_messages() {
        let mut rt = SimRuntime::new(42);
        let a = rt.add_node("alice");
        let b = rt.add_node("bob");

        // Before any messages, system should be converged (empty state)
        assert!(
            rt.check_convergence().is_converged(),
            "Empty system should be converged"
        );

        // Inject message with longer delay
        rt.inject_message(
            a,
            b,
            SyncMessage::SyncComplete { success: true },
            SimDuration::from_millis(100),
        );

        // With a message in flight, system should NOT be converged (C1 violated)
        assert!(
            !rt.check_convergence().is_converged(),
            "System with in-flight message should not be converged"
        );
        assert_eq!(
            rt.network().in_flight_count(),
            1,
            "Should have 1 message in flight"
        );

        // After processing the message, system should be converged again
        rt.step();
        assert!(
            rt.check_convergence().is_converged(),
            "System should be converged after message delivered"
        );
    }

    // =========================================================================
    // Network Tests
    // =========================================================================

    #[test]
    fn test_partition_blocks_messages() {
        let mut rt =
            SimRuntime::with_config(SimConfig::with_seed(42).with_faults(FaultConfig::none()));

        let a = rt.add_node("alice");
        let b = rt.add_node("bob");

        // Create partition immediately
        rt.schedule_partition(vec![vec![a.clone()], vec![b.clone()]], SimDuration::ZERO);
        rt.step(); // Process partition

        // Message should be dropped
        rt.inject_message(
            a,
            b,
            SyncMessage::SyncComplete { success: true },
            SimDuration::from_millis(10),
        );
        rt.step(); // Process message

        // Message was dropped
        assert_eq!(rt.network().metrics.messages_dropped_partition, 1);
    }

    #[test]
    fn test_fault_injection_loss() {
        let mut rt = SimRuntime::with_config(
            SimConfig::with_seed(42).with_faults(FaultConfig::none().with_loss(1.0)),
        );

        let a = rt.add_node("alice");
        let b = rt.add_node("bob");

        // All messages should be lost
        rt.inject_message(
            a,
            b,
            SyncMessage::SyncComplete { success: true },
            SimDuration::from_millis(10),
        );

        // Queue should be empty (message lost at send time)
        // Note: The inject_message uses node's next_message_id, then routes through network
        // But with 100% loss, message is dropped
    }

    // =========================================================================
    // Crash/Restart Tests
    // =========================================================================

    #[test]
    fn test_crash_preserves_storage() {
        let mut rt = SimRuntime::new(42);
        let a = rt.add_node("alice");

        // Add entity
        rt.node_mut(&a).unwrap().insert_entity(
            EntityId::from_u64(1),
            vec![1],
            CrdtType::LwwRegister,
        );

        // Crash
        rt.schedule_crash(a.clone(), SimDuration::ZERO);
        rt.step();

        // Storage preserved
        assert_eq!(rt.node(&a).unwrap().entity_count(), 1);
    }

    #[test]
    fn test_restart_increments_session() {
        let mut rt = SimRuntime::new(42);
        let a = rt.add_node("alice");

        assert_eq!(rt.node(&a).unwrap().session, 0);

        rt.schedule_crash(a.clone(), SimDuration::ZERO);
        rt.schedule_restart(a.clone(), SimDuration::from_millis(10));

        rt.step(); // crash
        rt.step(); // restart

        assert_eq!(rt.node(&a).unwrap().session, 1);
    }

    // =========================================================================
    // Random Scenario Tests
    // =========================================================================

    #[test]
    fn test_random_scenario_deterministic() {
        let nodes1 = RandomScenario::two_nodes_random(42);
        let nodes2 = RandomScenario::two_nodes_random(42);

        assert_eq!(nodes1.len(), nodes2.len());

        for (n1, n2) in nodes1.iter().zip(nodes2.iter()) {
            assert_eq!(n1.entity_count(), n2.entity_count());
        }
    }

    #[test]
    fn test_random_scenario_mesh() {
        let nodes = RandomScenario::mesh_random(42, 5);
        assert_eq!(nodes.len(), 5);
    }

    // =========================================================================
    // Metrics Tests
    // =========================================================================

    #[test]
    fn test_metrics_crash_counted() {
        let mut rt = SimRuntime::new(42);
        let a = rt.add_node("alice");

        rt.schedule_crash(a, SimDuration::ZERO);
        rt.step();

        assert_eq!(rt.metrics().effects.node_crashes, 1);
    }

    #[test]
    fn test_metrics_partition_counted() {
        let mut rt = SimRuntime::new(42);
        let a = rt.add_node("alice");
        let b = rt.add_node("bob");

        rt.schedule_partition(vec![vec![a], vec![b]], SimDuration::ZERO);
        rt.step();

        assert_eq!(rt.metrics().effects.partitions, 1);
    }

    // =========================================================================
    // Assertion Macro Tests
    // =========================================================================

    #[test]
    fn test_assert_macros() {
        let mut a = SimNode::new("a");
        let mut b = SimNode::new("b");

        // Empty nodes converged
        assert_converged!(a, b);

        // Add different entities
        a.insert_entity(EntityId::from_u64(1), vec![1], CrdtType::LwwRegister);
        b.insert_entity(EntityId::from_u64(2), vec![2], CrdtType::LwwRegister);

        assert_not_converged!(a, b);
        assert_entity_count!(a, 1);
        assert_has_entity!(a, EntityId::from_u64(1));
        assert_no_entity!(a, EntityId::from_u64(2));
        assert_idle!(a);
        assert_buffer_empty!(a);
    }
}
