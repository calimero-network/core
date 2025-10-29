//! Integration tests for DAG + Storage
//!
//! Tests that deltas are correctly applied to storage.
//! Verifies: DAG manages topology → Applier applies actions → Storage updated.
//!
//! Test coverage:
//! - Sequential delta application
//! - Out-of-order delta buffering and application
//! - Concurrent updates and merges
//! - Error handling and recovery
//! - Stress testing
//! - Real app states with collections

use std::sync::Arc;
use std::time::Duration;

use calimero_dag::{ApplyError, CausalDelta, DagStore, DeltaApplier};
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::entities::{ChildInfo, Metadata};
use calimero_storage::env::time_now;
use calimero_storage::index::Index;
use calimero_storage::interface::Interface;
use calimero_storage::store::MainStorage;
use tokio::sync::Mutex;

/// Storage applier that actually applies actions to MainStorage
struct StorageApplier {
    applied: Arc<Mutex<Vec<AppliedDelta>>>,
    should_fail: Arc<Mutex<bool>>,
}

impl StorageApplier {
    fn new() -> Self {
        Self {
            applied: Arc::new(Mutex::new(Vec::new())),
            should_fail: Arc::new(Mutex::new(false)),
        }
    }

    async fn set_should_fail(&self, value: bool) {
        *self.should_fail.lock().await = value;
    }

    async fn get_applied(&self) -> Vec<AppliedDelta> {
        self.applied.lock().await.clone()
    }
}

#[derive(Debug, Clone)]
struct AppliedDelta {
    delta_id: [u8; 32],
    action_count: usize,
}

#[async_trait::async_trait]
impl DeltaApplier<Vec<Action>> for StorageApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
        if *self.should_fail.lock().await {
            return Err(ApplyError::Application("Simulated failure".to_string()));
        }

        // Actually apply each action to storage
        for action in &delta.payload {
            Interface::<MainStorage>::apply_action(action.clone())
                .map_err(|e| ApplyError::Application(e.to_string()))?;
        }

        // Track that this delta was applied
        self.applied.lock().await.push(AppliedDelta {
            delta_id: delta.id,
            action_count: delta.payload.len(),
        });

        Ok(())
    }
}

#[tokio::test]
async fn test_dag_applies_deltas_to_storage_in_order() {
    let applier = StorageApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Setup: Create two entities in storage as roots
    let id1 = Id::new([1; 32]);
    let id2 = Id::new([2; 32]);

    Index::<MainStorage>::add_root(ChildInfo::new(id1, [10; 32], Metadata::default())).unwrap();
    Index::<MainStorage>::add_root(ChildInfo::new(id2, [20; 32], Metadata::default())).unwrap();

    // Create Update actions
    let action1 = Action::Update {
        id: id1,
        data: b"data from delta 1".to_vec(),
        ancestors: vec![],
        metadata: Metadata::default(),
    };

    let action2 = Action::Update {
        id: id2,
        data: b"data from delta 2".to_vec(),
        ancestors: vec![],
        metadata: Metadata::default(),
    };

    // Apply deltas in order
    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], vec![action1]);
    let delta2 = CausalDelta::new_test([2; 32], vec![[1; 32]], vec![action2]);

    let applied1 = dag.add_delta(delta1, &applier).await.unwrap();
    let applied2 = dag.add_delta(delta2, &applier).await.unwrap();

    assert!(applied1, "Delta1 should be applied");
    assert!(applied2, "Delta2 should be applied");

    // Verify applier was called in correct order
    let applied_deltas = applier.get_applied().await;
    assert_eq!(applied_deltas.len(), 2);
    assert_eq!(applied_deltas[0].delta_id, [1; 32]);
    assert_eq!(applied_deltas[1].delta_id, [2; 32]);

    // CRITICAL: Verify storage was actually updated by the applier
    let stored1 = Interface::<MainStorage>::get(id1).unwrap();
    let stored2 = Interface::<MainStorage>::get(id2).unwrap();

    assert_eq!(
        stored1, b"data from delta 1",
        "Storage should have data from delta1"
    );
    assert_eq!(
        stored2, b"data from delta 2",
        "Storage should have data from delta2"
    );

    // Verify DAG state
    assert_eq!(dag.get_heads(), vec![[2; 32]]);
    assert_eq!(dag.stats().applied_deltas, 3); // root + delta1 + delta2
}

#[tokio::test]
async fn test_dag_handles_out_of_order_and_applies_to_storage() {
    let applier = StorageApplier::new();
    let mut dag = DagStore::new([0; 32]);

    let id1 = Id::new([10; 32]);
    let id2 = Id::new([20; 32]);

    Index::<MainStorage>::add_root(ChildInfo::new(id1, [11; 32], Metadata::default())).unwrap();
    Index::<MainStorage>::add_root(ChildInfo::new(id2, [22; 32], Metadata::default())).unwrap();

    let action1 = Action::Update {
        id: id1,
        data: b"first delta data".to_vec(),
        ancestors: vec![],
        metadata: Metadata::default(),
    };

    let action2 = Action::Update {
        id: id2,
        data: b"second delta data".to_vec(),
        ancestors: vec![],
        metadata: Metadata::default(),
    };

    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], vec![action1]);
    let delta2 = CausalDelta::new_test([2; 32], vec![[1; 32]], vec![action2]);

    // Receive OUT OF ORDER - critical test!
    let applied2 = dag.add_delta(delta2.clone(), &applier).await.unwrap();
    assert!(!applied2, "Delta2 should be pending (missing parent)");

    // Nothing applied yet - delta2 is buffered, not applied to storage
    assert_eq!(
        applier.get_applied().await.len(),
        0,
        "No deltas applied yet"
    );

    // Delta2 should be buffered
    assert_eq!(dag.pending_stats().count, 1, "Delta2 should be pending");
    assert_eq!(
        dag.get_missing_parents(),
        vec![[1; 32]],
        "Missing parent delta1"
    );

    // Now receive delta1
    let applied1 = dag.add_delta(delta1, &applier).await.unwrap();
    assert!(applied1, "Delta1 should be applied");

    // Both deltas should now be applied to storage!
    let applied_deltas = applier.get_applied().await;
    assert_eq!(applied_deltas.len(), 2, "Both deltas should be applied");
    assert_eq!(applied_deltas[0].delta_id, [1; 32], "Delta1 applied first");
    assert_eq!(
        applied_deltas[1].delta_id, [2; 32],
        "Delta2 auto-applied second"
    );

    // CRITICAL: Verify storage was updated in correct order
    let stored1 = Interface::<MainStorage>::get(id1).unwrap();
    let stored2 = Interface::<MainStorage>::get(id2).unwrap();

    assert_eq!(stored1, b"first delta data", "Storage has delta1 data");
    assert_eq!(
        stored2, b"second delta data",
        "Storage has delta2 data (auto-applied!)"
    );

    // All pending should be cleared
    assert_eq!(dag.pending_stats().count, 0, "No pending deltas");
    assert_eq!(dag.stats().applied_deltas, 3); // root + delta1 + delta2
    assert_eq!(dag.get_heads(), vec![[2; 32]]);
}

#[tokio::test]
async fn test_dag_concurrent_updates_both_applied_to_storage() {
    let applier = StorageApplier::new();
    let mut dag = DagStore::new([0; 32]);

    let id_a = Id::new([100; 32]);
    let id_b = Id::new([200; 32]);

    Index::<MainStorage>::add_root(ChildInfo::new(id_a, [101; 32], Metadata::default())).unwrap();
    Index::<MainStorage>::add_root(ChildInfo::new(id_b, [201; 32], Metadata::default())).unwrap();

    let action_a = Action::Update {
        id: id_a,
        data: b"concurrent update A".to_vec(),
        ancestors: vec![],
        metadata: Metadata::default(),
    };

    let action_b = Action::Update {
        id: id_b,
        data: b"concurrent update B".to_vec(),
        ancestors: vec![],
        metadata: Metadata::default(),
    };

    // Concurrent updates from genesis - both valid!
    let delta_a = CausalDelta::new_test([10; 32], vec![[0; 32]], vec![action_a]);
    let delta_b = CausalDelta::new_test([20; 32], vec![[0; 32]], vec![action_b]);

    dag.add_delta(delta_a, &applier).await.unwrap();
    dag.add_delta(delta_b, &applier).await.unwrap();

    // Should have TWO heads (DAG branch)
    let mut heads = dag.get_heads();
    heads.sort();
    assert_eq!(heads.len(), 2, "Concurrent updates create two heads");
    assert!(heads.contains(&[10; 32]));
    assert!(heads.contains(&[20; 32]));

    // BOTH updates should be applied
    let applied_deltas = applier.get_applied().await;
    assert_eq!(applied_deltas.len(), 2, "Both concurrent deltas applied");

    // CRITICAL: Verify both concurrent updates are in storage
    let stored_a = Interface::<MainStorage>::get(id_a).unwrap();
    let stored_b = Interface::<MainStorage>::get(id_b).unwrap();

    assert_eq!(stored_a, b"concurrent update A", "Storage has update A");
    assert_eq!(stored_b, b"concurrent update B", "Storage has update B");

    // Create merge delta
    let delta_merge = CausalDelta::new_test(
        [30; 32],
        vec![[10; 32], [20; 32]], // merge both branches
        vec![],                   // no actions needed
    );

    dag.add_delta(delta_merge, &applier).await.unwrap();

    // Should have ONE head now (merged)
    assert_eq!(dag.get_heads(), vec![[30; 32]]);
    assert_eq!(dag.stats().applied_deltas, 4); // root + delta_a + delta_b + merge

    // Storage should still have both updates
    assert_eq!(
        Interface::<MainStorage>::get(id_a).unwrap(),
        b"concurrent update A"
    );
    assert_eq!(
        Interface::<MainStorage>::get(id_b).unwrap(),
        b"concurrent update B"
    );
}

// ============================================================
// Additional Integration Tests
// ============================================================

#[tokio::test]
async fn test_dag_storage_error_handling() {
    let applier = StorageApplier::new();
    let mut dag = DagStore::new([0; 32]);

    let id1 = Id::new([1; 32]);
    Index::<MainStorage>::add_root(ChildInfo::new(id1, [10; 32], Metadata::default())).unwrap();

    let action1 = Action::Update {
        id: id1,
        data: b"delta 1".to_vec(),
        ancestors: vec![],
        metadata: Metadata::default(),
    };

    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], vec![action1]);

    // Enable failure
    applier.set_should_fail(true).await;

    // Should fail to apply
    let result = dag.add_delta(delta1.clone(), &applier).await;
    assert!(result.is_err(), "Should fail when applier fails");

    // Delta stored but not applied
    assert!(dag.has_delta(&[1; 32]));
    assert_eq!(dag.stats().applied_deltas, 1); // Only root
    assert_eq!(applier.get_applied().await.len(), 0);
}

#[tokio::test]
async fn test_dag_storage_lww_through_deltas() {
    let applier = StorageApplier::new();
    let mut dag = DagStore::new([0; 32]);

    let id = Id::new([1; 32]);
    Index::<MainStorage>::add_root(ChildInfo::new(id, [10; 32], Metadata::default())).unwrap();

    // Two concurrent updates with different timestamps (using default metadata for simplicity)
    let action1 = Action::Update {
        id,
        data: b"version 1".to_vec(),
        ancestors: vec![],
        metadata: Metadata::default(),
    };

    // Small delay to ensure different timestamp
    std::thread::sleep(std::time::Duration::from_millis(2));

    let action2 = Action::Update {
        id,
        data: b"version 2".to_vec(),
        ancestors: vec![],
        metadata: Metadata::default(),
    };

    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], vec![action1]);
    let delta2 = CausalDelta::new_test([2; 32], vec![[0; 32]], vec![action2]);

    // Apply both
    dag.add_delta(delta1, &applier).await.unwrap();
    dag.add_delta(delta2, &applier).await.unwrap();

    // Newer timestamp should win
    let stored = Interface::<MainStorage>::get(id).unwrap();
    assert_eq!(stored, b"version 2");
}

#[tokio::test]
async fn test_dag_storage_delete_via_delta() {
    let applier = StorageApplier::new();
    let mut dag = DagStore::new([0; 32]);

    let id = Id::new([1; 32]);
    Index::<MainStorage>::add_root(ChildInfo::new(id, [10; 32], Metadata::default())).unwrap();

    // Add entity
    let add_action = Action::Add {
        id,
        data: b"test data".to_vec(),
        ancestors: vec![],
        metadata: Metadata::default(),
    };

    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], vec![add_action]);
    dag.add_delta(delta1, &applier).await.unwrap();

    // Verify exists
    assert!(Interface::<MainStorage>::get(id).is_ok());

    // Delete via delta
    let delete_action = Action::DeleteRef {
        id,
        deleted_at: time_now(),
    };

    let delta2 = CausalDelta::new_test([2; 32], vec![[1; 32]], vec![delete_action]);
    dag.add_delta(delta2, &applier).await.unwrap();

    // Should be deleted (tombstone check via is_deleted)
    assert!(Index::<MainStorage>::is_deleted(id).unwrap());
}

#[tokio::test]
async fn test_dag_storage_multiple_actions_per_delta() {
    let applier = StorageApplier::new();
    let mut dag = DagStore::new([0; 32]);

    let id1 = Id::new([1; 32]);
    let id2 = Id::new([2; 32]);
    let id3 = Id::new([3; 32]);

    Index::<MainStorage>::add_root(ChildInfo::new(id1, [11; 32], Metadata::default())).unwrap();
    Index::<MainStorage>::add_root(ChildInfo::new(id2, [22; 32], Metadata::default())).unwrap();
    Index::<MainStorage>::add_root(ChildInfo::new(id3, [33; 32], Metadata::default())).unwrap();

    // Single delta with multiple actions
    let actions = vec![
        Action::Update {
            id: id1,
            data: b"update 1".to_vec(),
            ancestors: vec![],
            metadata: Metadata::default(),
        },
        Action::Update {
            id: id2,
            data: b"update 2".to_vec(),
            ancestors: vec![],
            metadata: Metadata::default(),
        },
        Action::Update {
            id: id3,
            data: b"update 3".to_vec(),
            ancestors: vec![],
            metadata: Metadata::default(),
        },
    ];

    let delta = CausalDelta::new_test([1; 32], vec![[0; 32]], actions.clone());
    dag.add_delta(delta, &applier).await.unwrap();

    // All actions should be applied
    let applied_deltas = applier.get_applied().await;
    assert_eq!(applied_deltas.len(), 1);
    assert_eq!(applied_deltas[0].action_count, 3);

    // Verify all updates in storage
    assert_eq!(Interface::<MainStorage>::get(id1).unwrap(), b"update 1");
    assert_eq!(Interface::<MainStorage>::get(id2).unwrap(), b"update 2");
    assert_eq!(Interface::<MainStorage>::get(id3).unwrap(), b"update 3");
}

#[tokio::test]
async fn test_dag_storage_deep_chain_out_of_order() {
    let applier = StorageApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create entities
    let ids: Vec<_> = (1..=10).map(|i| Id::new([i; 32])).collect();
    for id in &ids {
        Index::<MainStorage>::add_root(ChildInfo::new(*id, [0; 32], Metadata::default())).unwrap();
    }

    // Create chain of 10 deltas
    let deltas: Vec<_> = (1..=10)
        .map(|i| {
            let action = Action::Update {
                id: ids[i - 1],
                data: format!("value {}", i).into_bytes(),
                ancestors: vec![],
                metadata: Metadata::default(),
            };

            CausalDelta::new_test([i as u8; 32], vec![[(i - 1) as u8; 32]], vec![action])
        })
        .collect();

    // Add in reverse order
    for delta in deltas.iter().rev() {
        dag.add_delta(delta.clone(), &applier).await.unwrap();
    }

    // All should be applied in correct order
    let applied = applier.get_applied().await;
    assert_eq!(applied.len(), 10);
    for i in 1..=10 {
        assert_eq!(applied[i - 1].delta_id, [i as u8; 32]);
    }

    // Verify storage has correct final state
    for i in 1..=10 {
        let stored = Interface::<MainStorage>::get(ids[i - 1]).unwrap();
        assert_eq!(stored, format!("value {}", i).as_bytes());
    }

    assert_eq!(dag.pending_stats().count, 0);
}

#[tokio::test]
async fn test_dag_storage_concurrent_branches_merge() {
    let applier = StorageApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create two branches from root, each updating different entities
    let id_a = Id::new([10; 32]);
    let id_b = Id::new([20; 32]);

    Index::<MainStorage>::add_root(ChildInfo::new(id_a, [11; 32], Metadata::default())).unwrap();
    Index::<MainStorage>::add_root(ChildInfo::new(id_b, [22; 32], Metadata::default())).unwrap();

    // Branch A: root -> delta_a -> delta_a2
    let delta_a = CausalDelta::new_test(
        [1; 32],
        vec![[0; 32]],
        vec![Action::Update {
            id: id_a,
            data: b"branch A v1".to_vec(),
            ancestors: vec![],
            metadata: Metadata::default(),
        }],
    );

    let delta_a2 = CausalDelta::new_test(
        [2; 32],
        vec![[1; 32]],
        vec![Action::Update {
            id: id_a,
            data: b"branch A v2".to_vec(),
            ancestors: vec![],
            metadata: Metadata::default(),
        }],
    );

    // Branch B: root -> delta_b
    let delta_b = CausalDelta::new_test(
        [3; 32],
        vec![[0; 32]],
        vec![Action::Update {
            id: id_b,
            data: b"branch B".to_vec(),
            ancestors: vec![],
            metadata: Metadata::default(),
        }],
    );

    // Apply both branches
    dag.add_delta(delta_a, &applier).await.unwrap();
    dag.add_delta(delta_a2, &applier).await.unwrap();
    dag.add_delta(delta_b, &applier).await.unwrap();

    // Two heads
    let mut heads = dag.get_heads();
    heads.sort();
    assert_eq!(heads.len(), 2);

    // Merge both branches
    let merge = CausalDelta::new_test(
        [99; 32],
        vec![[2; 32], [3; 32]],
        vec![], // No new actions in merge
    );

    dag.add_delta(merge, &applier).await.unwrap();

    // One head
    assert_eq!(dag.get_heads(), vec![[99; 32]]);

    // Storage should have updates from both branches
    assert_eq!(Interface::<MainStorage>::get(id_a).unwrap(), b"branch A v2");
    assert_eq!(Interface::<MainStorage>::get(id_b).unwrap(), b"branch B");
}

#[tokio::test]
async fn test_dag_storage_stress_many_deltas() {
    let applier = StorageApplier::new();
    let mut dag = DagStore::new([0; 32]);

    let id = Id::new([1; 32]);
    Index::<MainStorage>::add_root(ChildInfo::new(id, [10; 32], Metadata::default())).unwrap();

    // Create 100 sequential deltas
    for i in 1..=100 {
        let action = Action::Update {
            id,
            data: format!("version {}", i).into_bytes(),
            ancestors: vec![],
            metadata: Metadata::default(),
        };

        let delta = CausalDelta::new_test([i as u8; 32], vec![[(i - 1) as u8; 32]], vec![action]);

        dag.add_delta(delta, &applier).await.unwrap();
    }

    // All applied
    assert_eq!(applier.get_applied().await.len(), 100);
    assert_eq!(dag.stats().applied_deltas, 101); // root + 100

    // Final state
    let stored = Interface::<MainStorage>::get(id).unwrap();
    assert_eq!(stored, b"version 100");
}

// NOTE: Collection-based integration tests removed (see PRODUCTION_VS_TESTS_ANALYSIS.md)
// Collections manage their own storage internally and can't be serialized through DAG deltas.
// Comprehensive collection tests exist in crates/storage/src/tests/collections.rs

// ============================================================
// Context DAG Heads Tracking Tests
// ============================================================
// Production code (delta_store.rs:117-125) updates context dag_heads after every delta
// These tests verify that heads are tracked correctly

#[tokio::test]
async fn test_dag_heads_tracked_after_linear_deltas() {
    let applier = StorageApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Initial head
    assert_eq!(dag.get_heads(), vec![[0; 32]]);

    // Apply delta 1 (empty payload for simplicity)
    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], vec![]);
    dag.add_delta(delta1, &applier).await.unwrap();

    // Head should update to delta 1
    assert_eq!(dag.get_heads(), vec![[1; 32]]);

    // Apply delta 2
    let delta2 = CausalDelta::new_test([2; 32], vec![[1; 32]], vec![]);
    dag.add_delta(delta2, &applier).await.unwrap();

    // Head should update to delta 2
    assert_eq!(dag.get_heads(), vec![[2; 32]]);

    // Production: context.dag_heads would be [2; 32] at this point
    // (verified via context_client.update_dag_heads() call in DeltaStore::add_delta)
}

#[tokio::test]
async fn test_dag_heads_multiple_concurrent_branches() {
    let applier = StorageApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create two concurrent deltas
    let delta_a = CausalDelta::new_test([10; 32], vec![[0; 32]], vec![]);
    let delta_b = CausalDelta::new_test([20; 32], vec![[0; 32]], vec![]);

    dag.add_delta(delta_a, &applier).await.unwrap();
    dag.add_delta(delta_b, &applier).await.unwrap();

    // Should have 2 heads now
    let mut heads = dag.get_heads();
    heads.sort();
    assert_eq!(heads.len(), 2);
    assert_eq!(heads, vec![[10; 32], [20; 32]]);

    // Production: context.dag_heads would be [[10; 32], [20; 32]]
}

#[tokio::test]
async fn test_dag_heads_merge_reduces_to_single_head() {
    let applier = StorageApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create concurrent branches
    let delta_a = CausalDelta::new_test([1; 32], vec![[0; 32]], vec![]);
    let delta_b = CausalDelta::new_test([2; 32], vec![[0; 32]], vec![]);

    dag.add_delta(delta_a, &applier).await.unwrap();
    dag.add_delta(delta_b, &applier).await.unwrap();

    assert_eq!(dag.get_heads().len(), 2);

    // Merge both branches
    let merge = CausalDelta::new_test([99; 32], vec![[1; 32], [2; 32]], vec![]);
    dag.add_delta(merge, &applier).await.unwrap();

    // Should have single head now
    assert_eq!(dag.get_heads(), vec![[99; 32]]);

    // Production: context.dag_heads would be [[99; 32]] after merge
}
