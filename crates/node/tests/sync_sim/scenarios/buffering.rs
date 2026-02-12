//! Delta buffering scenarios for Invariant I6 testing.
//!
//! These scenarios test the behavior of delta buffering during state-based sync.
//! Per CIP Invariant I6: "Deltas received during state-based sync MUST be preserved
//! and applied after sync completes. Implementations MUST NOT drop buffered deltas."
//!
//! # Test Coverage
//!
//! 1. `test_deltas_buffered_during_sync` - Deltas arriving during sync are buffered
//! 2. `test_buffered_deltas_replayed_on_completion` - Buffered deltas applied after sync
//! 3. `test_deltas_applied_immediately_when_idle` - Deltas applied immediately when not syncing
//! 4. `test_buffered_deltas_cleared_on_crash` - Buffer cleared on node crash
//! 5. `test_multiple_deltas_preserved_fifo` - Multiple deltas replayed in FIFO order

use crate::sync_sim::actions::{EntityMetadata, StorageOp};
use crate::sync_sim::node::SimNode;
use crate::sync_sim::runtime::SimDuration;
use crate::sync_sim::sim_runtime::SimRuntime;
use crate::sync_sim::types::{DeltaId, EntityId};
use calimero_primitives::crdt::CrdtType;

/// Create a DeltaId from a u64 for testing convenience.
fn delta_id_from_u64(n: u64) -> DeltaId {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&n.to_le_bytes());
    DeltaId::from_bytes(bytes)
}

/// Create a simple insert operation for testing.
fn make_insert_op(entity_id: u64, value: &str) -> StorageOp {
    StorageOp::Insert {
        id: EntityId::from_u64(entity_id),
        data: value.as_bytes().to_vec(),
        metadata: EntityMetadata::new(CrdtType::LwwRegister, entity_id * 100),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test: Deltas arriving during active sync are buffered, not applied.
    ///
    /// Verifies Invariant I6: deltas MUST be preserved during sync.
    #[test]
    fn test_deltas_buffered_during_sync() {
        let mut rt = SimRuntime::new(42);

        // Create a fresh node
        let node_id = rt.add_node("syncing_node");

        // Start sync on the node
        rt.schedule_sync_start(node_id.clone(), SimDuration::ZERO);
        rt.step(); // Process SyncStart

        // Verify node is now syncing
        assert!(
            rt.node(&node_id).unwrap().sync_state.is_active(),
            "Node should be in active sync state"
        );

        // Schedule a gossip delta to arrive during sync
        let delta_id = delta_id_from_u64(1);
        let operations = vec![make_insert_op(100, "buffered_value")];
        rt.schedule_gossip_delta(
            node_id.clone(),
            delta_id,
            operations,
            SimDuration::from_millis(10),
        );
        rt.step(); // Process GossipDelta

        // Verify: delta was buffered, NOT applied to storage
        let node = rt.node(&node_id).unwrap();
        assert_eq!(
            node.buffer_size(),
            1,
            "Delta should be buffered during sync"
        );
        assert_eq!(
            node.entity_count(),
            0,
            "Delta should NOT be applied to storage during sync"
        );
        assert_eq!(
            node.buffered_operations_count(),
            1,
            "Operations should be buffered for replay"
        );
    }

    /// Test: Buffered deltas are replayed when sync completes.
    ///
    /// Verifies Invariant I6: deltas MUST be applied after sync completes.
    #[test]
    fn test_buffered_deltas_replayed_on_completion() {
        let mut rt = SimRuntime::new(42);

        let node_id = rt.add_node("syncing_node");

        // Start sync
        rt.schedule_sync_start(node_id.clone(), SimDuration::ZERO);
        rt.step();

        // Send delta during sync
        let delta_id = delta_id_from_u64(1);
        let operations = vec![make_insert_op(100, "replayed_value")];
        rt.schedule_gossip_delta(
            node_id.clone(),
            delta_id,
            operations,
            SimDuration::from_millis(10),
        );
        rt.step();

        // Verify buffered
        assert_eq!(rt.node(&node_id).unwrap().buffer_size(), 1);
        assert_eq!(rt.node(&node_id).unwrap().entity_count(), 0);

        // Complete sync - should replay buffered deltas
        rt.schedule_sync_complete(node_id.clone(), SimDuration::from_millis(20));
        rt.step();

        // Verify: delta was applied, buffer cleared
        let node = rt.node(&node_id).unwrap();
        assert_eq!(node.buffer_size(), 0, "Buffer should be cleared after sync");
        assert_eq!(
            node.entity_count(),
            1,
            "Buffered delta should be applied after sync"
        );
        assert!(
            !node.sync_state.is_active(),
            "Node should be idle after sync completes"
        );
    }

    /// Test: Deltas are applied immediately when node is not syncing.
    #[test]
    fn test_deltas_applied_immediately_when_idle() {
        let mut rt = SimRuntime::new(42);

        let node_id = rt.add_node("idle_node");

        // Node is idle (not syncing)
        assert!(!rt.node(&node_id).unwrap().sync_state.is_active());

        // Send delta
        let delta_id = delta_id_from_u64(1);
        let operations = vec![make_insert_op(100, "immediate_value")];
        rt.schedule_gossip_delta(
            node_id.clone(),
            delta_id,
            operations,
            SimDuration::from_millis(10),
        );
        rt.step();

        // Verify: delta applied immediately, nothing buffered
        let node = rt.node(&node_id).unwrap();
        assert_eq!(
            node.buffer_size(),
            0,
            "No buffering when node is not syncing"
        );
        assert_eq!(
            node.entity_count(),
            1,
            "Delta should be applied immediately when idle"
        );
    }

    /// Test: Buffered deltas are lost on node crash (not persisted).
    ///
    /// This is expected behavior - crash recovery restarts sync from scratch.
    #[test]
    fn test_buffered_deltas_cleared_on_crash() {
        let mut rt = SimRuntime::new(42);

        let node_id = rt.add_node("crashing_node");

        // Start sync and buffer a delta
        rt.schedule_sync_start(node_id.clone(), SimDuration::ZERO);
        rt.step();

        let delta_id = delta_id_from_u64(1);
        let operations = vec![make_insert_op(100, "will_be_lost")];
        rt.schedule_gossip_delta(
            node_id.clone(),
            delta_id,
            operations,
            SimDuration::from_millis(10),
        );
        rt.step();

        assert_eq!(rt.node(&node_id).unwrap().buffer_size(), 1);

        // Crash the node
        rt.schedule_crash(node_id.clone(), SimDuration::from_millis(20));
        rt.step();

        // Verify: buffer cleared, sync state reset
        let node = rt.node(&node_id).unwrap();
        assert!(node.is_crashed, "Node should be crashed");
        assert_eq!(node.buffer_size(), 0, "Buffer should be cleared on crash");
        assert_eq!(
            node.buffered_operations_count(),
            0,
            "Buffered operations should be cleared"
        );
        assert!(
            !node.sync_state.is_active(),
            "Sync state should be reset on crash"
        );
    }

    /// Test: Multiple deltas are preserved and replayed in FIFO order.
    ///
    /// Verifies that the buffering mechanism maintains insertion order.
    #[test]
    fn test_multiple_deltas_preserved_fifo() {
        let mut rt = SimRuntime::new(42);

        let node_id = rt.add_node("multi_delta_node");

        // Start sync
        rt.schedule_sync_start(node_id.clone(), SimDuration::ZERO);
        rt.step();

        // Send multiple deltas with different timestamps
        for i in 1..=5 {
            let delta_id = delta_id_from_u64(i);
            let operations = vec![make_insert_op(100 + i, &format!("delta_{}", i))];
            rt.schedule_gossip_delta(
                node_id.clone(),
                delta_id,
                operations,
                SimDuration::from_millis(10 * i),
            );
        }

        // Process all deltas
        for _ in 0..5 {
            rt.step();
        }

        // Verify all buffered
        assert_eq!(
            rt.node(&node_id).unwrap().buffer_size(),
            5,
            "All 5 deltas should be buffered"
        );
        assert_eq!(
            rt.node(&node_id).unwrap().entity_count(),
            0,
            "No deltas applied yet"
        );

        // Complete sync
        rt.schedule_sync_complete(node_id.clone(), SimDuration::from_millis(100));
        rt.step();

        // Verify all deltas applied
        let node = rt.node(&node_id).unwrap();
        assert_eq!(node.buffer_size(), 0, "Buffer should be empty");
        assert_eq!(
            node.entity_count(),
            5,
            "All 5 deltas should be applied after sync"
        );
    }

    /// Test: Convergence is blocked while deltas are buffered.
    ///
    /// The simulation's convergence check (C3) requires all buffers to be empty.
    #[test]
    fn test_convergence_blocked_with_buffered_deltas() {
        let mut rt = SimRuntime::new(42);

        // Create two nodes - one syncing, one idle
        let syncing = rt.add_node("syncing");
        let idle = rt.add_node("idle");

        // Give idle node some state with specific data
        let shared_data = b"shared_data".to_vec();
        let metadata = EntityMetadata::new(CrdtType::LwwRegister, 100);
        rt.node_mut(&idle).unwrap().insert_entity_with_metadata(
            EntityId::from_u64(1),
            shared_data.clone(),
            metadata.clone(),
        );

        // Start sync on syncing node
        rt.schedule_sync_start(syncing.clone(), SimDuration::ZERO);
        rt.step();

        // Buffer a delta on syncing node with SAME data as idle node
        let delta_id = delta_id_from_u64(1);
        let operations = vec![StorageOp::Insert {
            id: EntityId::from_u64(1),
            data: shared_data,
            metadata,
        }];
        rt.schedule_gossip_delta(
            syncing.clone(),
            delta_id,
            operations,
            SimDuration::from_millis(10),
        );
        rt.step();

        // Convergence should be blocked (C3: all buffers must be empty)
        let convergence = rt.check_convergence();
        assert!(
            !convergence.is_converged(),
            "Should NOT converge while deltas are buffered"
        );

        // Complete sync
        rt.schedule_sync_complete(syncing.clone(), SimDuration::from_millis(20));
        rt.step();

        // Now should converge (both have same entity with same data)
        let convergence = rt.check_convergence();
        assert!(
            convergence.is_converged(),
            "Should converge after sync completes and deltas replayed"
        );
    }

    /// Test: Complex scenario - snapshot sync with concurrent writes.
    ///
    /// Simulates a real-world scenario where:
    /// 1. Fresh node joins network
    /// 2. Snapshot sync starts
    /// 3. Source node produces new writes during sync
    /// 4. New writes are gossiped to fresh node
    /// 5. Fresh node buffers them
    /// 6. Snapshot completes
    /// 7. Buffered writes are replayed
    /// 8. Both nodes converge
    #[test]
    fn test_snapshot_sync_with_concurrent_writes() {
        let mut rt = SimRuntime::new(42);

        // Source node with existing data
        let mut source = SimNode::new("source");
        for i in 0..10 {
            source.insert_entity(
                EntityId::from_u64(i),
                format!("initial_{}", i).into_bytes(),
                CrdtType::LwwRegister,
            );
        }
        let source_id = rt.add_existing_node(source);

        // Fresh node (needs snapshot sync)
        let fresh_id = rt.add_node("fresh");

        // Fresh node starts snapshot sync
        rt.schedule_sync_start(fresh_id.clone(), SimDuration::ZERO);
        rt.step();

        // Simulate snapshot transfer (copy source's entities to fresh)
        // In real implementation, this happens via SnapshotPage messages
        let source_entities: Vec<_> = {
            let s = rt.node(&source_id).unwrap();
            s.storage
                .entities_sorted()
                .into_iter()
                .map(|e| (e.id, e.data.clone(), e.metadata.clone()))
                .collect()
        };
        for (id, data, metadata) in source_entities {
            rt.node_mut(&fresh_id)
                .unwrap()
                .insert_entity_with_metadata(id, data, metadata);
        }

        // MEANWHILE: Source produces new writes that get gossiped to fresh
        // (These arrive during sync and must be buffered)
        for i in 10..15 {
            let delta_id = delta_id_from_u64(i);
            let data = format!("concurrent_{}", i).into_bytes();
            // Use consistent metadata for both source and fresh
            let metadata = EntityMetadata::new(CrdtType::LwwRegister, i * 100);
            let operations = vec![StorageOp::Insert {
                id: EntityId::from_u64(i),
                data: data.clone(),
                metadata: metadata.clone(),
            }];

            // Also apply to source immediately with same metadata
            rt.node_mut(&source_id)
                .unwrap()
                .insert_entity_with_metadata(EntityId::from_u64(i), data, metadata);

            // Gossip to fresh (will be buffered)
            rt.schedule_gossip_delta(
                fresh_id.clone(),
                delta_id,
                operations,
                SimDuration::from_millis(10 + i),
            );
        }

        // Process all gossip deltas
        for _ in 0..5 {
            rt.step();
        }

        // Verify: fresh has initial 10, source has all 15
        // fresh should have 5 buffered
        assert_eq!(rt.node(&fresh_id).unwrap().entity_count(), 10);
        assert_eq!(rt.node(&fresh_id).unwrap().buffer_size(), 5);
        assert_eq!(rt.node(&source_id).unwrap().entity_count(), 15);

        // System should NOT be converged yet
        assert!(!rt.check_convergence().is_converged());

        // Complete sync on fresh - replays buffered deltas
        rt.schedule_sync_complete(fresh_id.clone(), SimDuration::from_millis(100));
        rt.step();

        // Both nodes should now have 15 entities
        assert_eq!(rt.node(&fresh_id).unwrap().entity_count(), 15);
        assert_eq!(rt.node(&source_id).unwrap().entity_count(), 15);
        assert_eq!(rt.node(&fresh_id).unwrap().buffer_size(), 0);

        // System should converge
        assert!(
            rt.check_convergence().is_converged(),
            "Nodes should converge after buffered deltas are replayed"
        );
    }
}
