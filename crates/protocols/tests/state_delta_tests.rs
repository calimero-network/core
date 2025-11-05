//! Tests for state_delta handler (gossipsub)

mod common;

use common::{create_test_context_id, create_test_delta, create_test_identity, mocks::MockDeltaStore};
use calimero_protocols::gossipsub::state_delta::{handle_state_delta, DeltaStore};

/// Test that state_delta handler exists
#[test]
fn test_state_delta_handler_exists() {
    // Validate that the handler function exists
    let _ = handle_state_delta as usize;
}

#[tokio::test]
async fn test_delta_store_integration_with_state_delta() {
    // Test that our MockDeltaStore works with the state_delta handler's expectations
    let store = MockDeltaStore::new();
    
    // Create test delta
    let delta = create_test_delta(vec![[0; 32]]);
    let delta_id = delta.id;
    
    // Add delta (simulating what state_delta handler would do)
    let result = store.add_delta_with_events(delta, None).await.unwrap();
    
    // Verify delta was added
    assert!(result.applied);
    assert!(store.has_delta(&delta_id).await);
}

#[tokio::test]
async fn test_delta_cascade_logic() {
    let store = MockDeltaStore::new();
    
    // Add genesis delta
    let genesis = create_test_delta(vec![[0; 32]]);
    let genesis_id = genesis.id;
    store.add_delta(genesis).await.unwrap();
    
    // Add child delta
    let child = create_test_delta(vec![genesis_id]);
    let child_id = child.id;
    store.add_delta(child).await.unwrap();
    
    // Both should be applied
    assert!(store.is_applied(&genesis_id));
    assert!(store.is_applied(&child_id));
}

#[tokio::test]
async fn test_missing_parent_detection() {
    let store = MockDeltaStore::new();
    
    // Simulate missing parents
    let missing = vec![[1; 32], [2; 32]];
    store.set_missing_parents(missing.clone());
    
    // Handler would detect these
    let result = store.get_missing_parents().await;
    assert_eq!(result.missing_ids.len(), 2);
    assert!(result.missing_ids.contains(&[1; 32]));
    assert!(result.missing_ids.contains(&[2; 32]));
}

#[test]
fn test_nonce_generation() {
    use rand::Rng;
    
    // Test nonce generation (used in encryption/decryption)
    let nonce1: calimero_crypto::Nonce = rand::thread_rng().gen();
    let nonce2: calimero_crypto::Nonce = rand::thread_rng().gen();
    
    // Nonces should be different
    assert_ne!(nonce1, nonce2);
}

#[test]
fn test_hash_types() {
    // Test hash creation for root_hash comparisons
    let hash1 = calimero_primitives::hash::Hash::from([0; 32]);
    let hash2 = calimero_primitives::hash::Hash::from([0; 32]);
    let hash3 = calimero_primitives::hash::Hash::from([1; 32]);
    
    assert_eq!(hash1, hash2);
    assert_ne!(hash1, hash3);
}

// TODO: Add integration tests for:
// - Full state_delta handling flow
// - Event handler execution
// - WebSocket event emission
// - Key share requests for missing sender_keys
// - Delta decryption and validation
// - Cascade execution of pending deltas
//
// These require:
// - Mock NodeClient (for events, blobs)
// - Mock ContextClient (for contexts, identities)
// - Mock NetworkClient (for key exchange)
// - Full DeltaStore implementation (or enhanced mock)

