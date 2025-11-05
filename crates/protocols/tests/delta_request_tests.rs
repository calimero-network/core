//! Tests for delta_request protocol

mod common;

use calimero_protocols::p2p::delta_request::DeltaStore;
use common::mocks::MockDeltaStore;
use common::create_test_delta;

#[tokio::test]
async fn test_delta_store_basic_operations() {
    let store = MockDeltaStore::new();
    
    // Create test delta
    let delta = create_test_delta(vec![[0; 32]]); // Genesis parent
    let delta_id = delta.id;
    
    // Should not exist initially
    assert!(!store.has_delta(&delta_id).await);
    
    // Add delta
    store.add_delta(delta.clone()).await.unwrap();
    
    // Should exist now
    assert!(store.has_delta(&delta_id).await);
    
    // Should be applied
    assert!(store.is_applied(&delta_id));
    
    // Should be retrievable
    let retrieved = store.get_delta(&delta_id).await;
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().id, delta_id);
}

#[tokio::test]
async fn test_delta_store_add_with_events() {
    let store = MockDeltaStore::new();
    
    // Create test delta
    let delta = create_test_delta(vec![[0; 32]]);
    let delta_id = delta.id;
    
    // Create test events
    let events = vec![1, 2, 3, 4, 5];
    
    // Add delta with events
    let result = store
        .add_delta_with_events(delta.clone(), Some(events.clone()))
        .await
        .unwrap();
    
    // Should be applied
    assert!(result.applied);
    
    // Should exist in store
    assert!(store.has_delta(&delta_id).await);
    assert!(store.dag_has_delta_applied(&delta_id).await);
}

#[tokio::test]
async fn test_delta_store_missing_parents() {
    let store = MockDeltaStore::new();
    
    // Simulate missing parents
    let missing_ids = vec![[1; 32], [2; 32], [3; 32]];
    store.set_missing_parents(missing_ids.clone());
    
    // Get missing parents
    let result = store.get_missing_parents().await;
    
    // Should match what we set
    assert_eq!(result.missing_ids, missing_ids);
}

#[tokio::test]
async fn test_delta_store_cascading() {
    let store = MockDeltaStore::new();
    
    // Add parent delta first
    let parent = create_test_delta(vec![[0; 32]]);
    let parent_id = parent.id;
    store.add_delta(parent).await.unwrap();
    
    // Add child delta
    let child = create_test_delta(vec![parent_id]);
    let child_id = child.id;
    store.add_delta(child).await.unwrap();
    
    // Both should be applied
    assert!(store.is_applied(&parent_id));
    assert!(store.is_applied(&child_id));
}

// Note: Full protocol tests (request_missing_deltas, handle_delta_request)
// require mock NetworkClient and ContextClient which are more complex.
// These basic tests validate the DeltaStore trait implementation.
// Full integration tests can be added once we have complete mocks.

