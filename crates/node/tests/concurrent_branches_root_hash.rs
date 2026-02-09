//! Integration test for concurrent branches with deterministic root hash selection
//!
//! This test verifies that when multiple DAG heads exist (concurrent branches),
//! the expected_root_hash field is properly tracked and used.

use calimero_dag::CausalDelta;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::entities::Metadata;

/// Test that concurrent branches maintain expected_root_hash field
///
/// Scenario:
///          → Delta A (id: [0x01...], expected_root_hash: [0xAA...]) ↘
///   Root                                                               → 2 heads
///          → Delta B (id: [0x02...], expected_root_hash: [0xBB...]) ↗
///
/// This test verifies that expected_root_hash is properly stored and can be
/// retrieved for each delta. The actual deterministic selection happens in
/// DeltaStore which requires full node/context setup.
#[tokio::test]
async fn test_concurrent_branches_track_expected_root_hash() {
    use calimero_dag::{ApplyError, DagStore, DeltaApplier};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    // Simple test applier that doesn't actually apply to storage
    struct TestApplier {
        applied: Arc<Mutex<Vec<([u8; 32], [u8; 32])>>>, // (delta_id, expected_root_hash)
    }

    #[async_trait::async_trait]
    impl DeltaApplier<Vec<Action>> for TestApplier {
        async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
            self.applied
                .lock()
                .await
                .push((delta.id, delta.expected_root_hash));
            Ok(())
        }
    }

    let applier = TestApplier {
        applied: Arc::new(Mutex::new(Vec::new())),
    };

    let mut dag = DagStore::new([0; 32]);

    // Create two concurrent deltas with different expected_root_hashes
    let delta_a = create_delta_with_root([0x01; 32], vec![[0; 32]], [0xAA; 32]);
    let delta_b = create_delta_with_root([0x02; 32], vec![[0; 32]], [0xBB; 32]);

    // Apply both
    let _ = dag.add_delta(delta_a.clone(), &applier).await.unwrap();
    let _ = dag.add_delta(delta_b.clone(), &applier).await.unwrap();

    // Should have TWO heads
    let mut heads = dag.get_heads();
    heads.sort();
    assert_eq!(heads.len(), 2);
    assert_eq!(heads, vec![[0x01; 32], [0x02; 32]]);

    // Verify applier received correct expected_root_hashes
    let applied = applier.applied.lock().await;
    assert_eq!(applied.len(), 2);

    // Find the applied delta with id [0x01; 32]
    let delta_a_applied = applied.iter().find(|(id, _)| *id == [0x01; 32]).unwrap();
    assert_eq!(
        delta_a_applied.1, [0xAA; 32],
        "Delta A should have expected_root_hash [0xAA; 32]"
    );

    // Find the applied delta with id [0x02; 32]
    let delta_b_applied = applied.iter().find(|(id, _)| *id == [0x02; 32]).unwrap();
    assert_eq!(
        delta_b_applied.1, [0xBB; 32],
        "Delta B should have expected_root_hash [0xBB; 32]"
    );
}

/// Test that merge delta correctly uses its own expected_root_hash
#[tokio::test]
async fn test_merge_delta_expected_root_hash() {
    use calimero_dag::{ApplyError, DagStore, DeltaApplier};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    struct TestApplier {
        applied: Arc<Mutex<Vec<([u8; 32], [u8; 32])>>>,
    }

    #[async_trait::async_trait]
    impl DeltaApplier<Vec<Action>> for TestApplier {
        async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
            self.applied
                .lock()
                .await
                .push((delta.id, delta.expected_root_hash));
            Ok(())
        }
    }

    let applier = TestApplier {
        applied: Arc::new(Mutex::new(Vec::new())),
    };

    let mut dag = DagStore::new([0; 32]);

    // Create concurrent branches
    let delta_a = create_delta_with_root([0x01; 32], vec![[0; 32]], [0xAA; 32]);
    let delta_b = create_delta_with_root([0x02; 32], vec![[0; 32]], [0xBB; 32]);

    let _ = dag.add_delta(delta_a, &applier).await.unwrap();
    let _ = dag.add_delta(delta_b, &applier).await.unwrap();

    // Create merge delta with its own expected_root_hash
    let delta_merge = create_delta_with_root(
        [0x03; 32],
        vec![[0x01; 32], [0x02; 32]],
        [0xCC; 32], // Merge produces new root_hash
    );

    let _ = dag.add_delta(delta_merge, &applier).await.unwrap();

    // Should have single head
    assert_eq!(dag.get_heads(), vec![[0x03; 32]]);

    // Verify merge delta had correct expected_root_hash
    let applied = applier.applied.lock().await;
    let merge_applied = applied.iter().find(|(id, _)| *id == [0x03; 32]).unwrap();
    assert_eq!(
        merge_applied.1, [0xCC; 32],
        "Merge delta should have its own expected_root_hash"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test Helpers
// ═══════════════════════════════════════════════════════════════════════

fn create_delta_with_root(
    id: [u8; 32],
    parents: Vec<[u8; 32]>,
    expected_root_hash: [u8; 32],
) -> CausalDelta<Vec<Action>> {
    // Create a simple action for testing
    let action = Action::Add {
        id: Id::from([id[0]; 32]),
        data: vec![1, 2, 3],
        ancestors: vec![],
        metadata: Metadata::default(),
    };

    CausalDelta {
        id,
        parents,
        payload: vec![action],
        hlc: calimero_storage::logical_clock::HybridTimestamp::default(),
        expected_root_hash,
        kind: calimero_dag::DeltaKind::Regular,
    }
}
