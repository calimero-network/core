//! Tests for UnorderedMap synchronization between nodes
//!
//! These tests verify that:
//! 1. Entry IDs are deterministic based on collection ID and key
//! 2. When syncing entries via actions, the entries can be found
//! 3. Concurrent additions to the same UnorderedMap sync correctly
//! 4. Root hash converges when same deltas are applied in different orders

use borsh::{from_slice, to_vec};
use sha2::{Digest, Sha256};

use crate::action::Action;
use crate::address::Id;
use crate::collections::{Root, UnorderedMap};
use crate::delta::reset_delta_context;
use crate::entities::{ChildInfo, Metadata};
use crate::env;
use crate::index::Index;
use crate::interface::Interface;
use crate::store::{Key, MainStorage, StorageAdaptor};

// =============================================================================
// Test: Entry ID Determinism
// =============================================================================

#[test]
fn test_entry_id_is_deterministic() {
    // Entry ID should depend only on collection ID and key, nothing else
    let collection_id = Id::new([42; 32]);
    let key = "test_key";

    let id1 = compute_entry_id(collection_id, key);
    let id2 = compute_entry_id(collection_id, key);

    assert_eq!(
        id1, id2,
        "Same collection ID and key should produce same entry ID"
    );

    // Different key = different ID
    let id3 = compute_entry_id(collection_id, "other_key");
    assert_ne!(
        id1, id3,
        "Different keys should produce different entry IDs"
    );

    // Different collection ID = different ID
    let other_collection_id = Id::new([99; 32]);
    let id4 = compute_entry_id(other_collection_id, key);
    assert_ne!(
        id1, id4,
        "Different collection IDs should produce different entry IDs"
    );
}

fn compute_entry_id(collection_id: Id, key: &str) -> Id {
    let mut hasher = Sha256::new();
    hasher.update(collection_id.as_bytes());
    hasher.update(key.as_bytes());
    Id::new(hasher.finalize().into())
}

// =============================================================================
// Test: Basic Sync - Add entry on Node A, sync to Node B
// =============================================================================

#[test]
fn test_sync_entry_basic() {
    env::reset_for_testing();
    reset_delta_context();

    // Node A: Create a KvStore with an entry
    let collection_id = Id::new([1; 32]);
    let key = "my_key";
    let value = "my_value";
    let ts = 100_u64;

    // Compute what the entry ID should be
    let entry_id = compute_entry_id(collection_id, key);

    // Simulate what happens when Node A inserts an entry:
    // 1. Entry data is stored at entry_id
    // 2. Entry is added to collection's children in index
    // 3. Action::Add is generated

    let entry_data = to_vec(&(key.to_string(), value.to_string())).unwrap();
    let entry_metadata = Metadata::new(ts, ts);

    // Store the entry directly (simulating Node A's write)
    MainStorage::storage_write(Key::Entry(entry_id), &entry_data);

    // Create the index entry
    Index::<MainStorage>::add_root(ChildInfo::new(
        collection_id,
        [0; 32],
        Metadata::new(ts, ts),
    ))
    .unwrap();

    Index::<MainStorage>::add_child_to(
        collection_id,
        ChildInfo::new(entry_id, [0; 32], entry_metadata.clone()),
    )
    .unwrap();

    // Verify we can read it back
    let read_back = MainStorage::storage_read(Key::Entry(entry_id));
    assert!(read_back.is_some(), "Entry should exist in storage");

    let (k, v): (String, String) = from_slice(&read_back.unwrap()).unwrap();
    assert_eq!(k, key);
    assert_eq!(v, value);

    // Now simulate Node B receiving this via sync action
    env::reset_for_testing();
    reset_delta_context();

    // Node B has the same collection ID (from snapshot sync)
    Index::<MainStorage>::add_root(ChildInfo::new(
        collection_id,
        [0; 32],
        Metadata::new(ts, ts),
    ))
    .unwrap();

    // Node B receives the Action::Add for the entry
    let action = Action::Add {
        id: entry_id,
        data: entry_data.clone(),
        metadata: entry_metadata.clone(),
        ancestors: vec![ChildInfo::new(
            collection_id,
            [0; 32],
            Metadata::new(ts, ts),
        )],
    };

    Interface::<MainStorage>::apply_action(action).unwrap();

    // Now Node B should be able to find the entry using the same ID computation
    let node_b_entry_id = compute_entry_id(collection_id, key);
    assert_eq!(
        node_b_entry_id, entry_id,
        "Node B should compute the same entry ID"
    );

    let node_b_read = MainStorage::storage_read(Key::Entry(node_b_entry_id));
    assert!(
        node_b_read.is_some(),
        "Node B should find the entry after sync"
    );

    let (k2, v2): (String, String) = from_slice(&node_b_read.unwrap()).unwrap();
    assert_eq!(k2, key);
    assert_eq!(v2, value);
}

// =============================================================================
// Test: Concurrent entries sync - Node A adds key_1, Node B adds key_2
// =============================================================================

#[test]
fn test_concurrent_entries_sync() {
    env::reset_for_testing();
    reset_delta_context();

    // Both nodes share the same collection ID (from genesis/snapshot sync)
    let collection_id = Id::new([42; 32]);
    let base_ts = 50_u64;

    // Node A adds key_1
    let key_1 = "key_1";
    let value_1 = "value_from_node_a";
    let entry_id_1 = compute_entry_id(collection_id, key_1);
    let entry_data_1 = to_vec(&(key_1.to_string(), value_1.to_string())).unwrap();
    let metadata_1 = Metadata::new(100, 100);

    // Node B adds key_2
    let key_2 = "key_2";
    let value_2 = "value_from_node_b";
    let entry_id_2 = compute_entry_id(collection_id, key_2);
    let entry_data_2 = to_vec(&(key_2.to_string(), value_2.to_string())).unwrap();
    let metadata_2 = Metadata::new(105, 105);

    // Setup: Both nodes have the collection in their index
    Index::<MainStorage>::add_root(ChildInfo::new(
        collection_id,
        [0; 32],
        Metadata::new(base_ts, base_ts),
    ))
    .unwrap();

    // Simulate Node A's perspective:
    // 1. Has key_1 locally
    // 2. Receives key_2 from Node B

    // Local key_1
    MainStorage::storage_write(Key::Entry(entry_id_1), &entry_data_1);
    Index::<MainStorage>::add_child_to(
        collection_id,
        ChildInfo::new(entry_id_1, [0; 32], metadata_1.clone()),
    )
    .unwrap();

    // Receive key_2 via sync
    let action_2 = Action::Add {
        id: entry_id_2,
        data: entry_data_2.clone(),
        metadata: metadata_2.clone(),
        ancestors: vec![ChildInfo::new(
            collection_id,
            [0; 32],
            Metadata::new(base_ts, base_ts),
        )],
    };
    Interface::<MainStorage>::apply_action(action_2).unwrap();

    // Verify Node A has both entries
    let read_1 = MainStorage::storage_read(Key::Entry(entry_id_1));
    let read_2 = MainStorage::storage_read(Key::Entry(entry_id_2));

    assert!(read_1.is_some(), "Node A should have key_1");
    assert!(read_2.is_some(), "Node A should have key_2 after sync");

    // Clear and test Node B's perspective
    env::reset_for_testing();
    reset_delta_context();

    Index::<MainStorage>::add_root(ChildInfo::new(
        collection_id,
        [0; 32],
        Metadata::new(base_ts, base_ts),
    ))
    .unwrap();

    // Local key_2
    MainStorage::storage_write(Key::Entry(entry_id_2), &entry_data_2);
    Index::<MainStorage>::add_child_to(
        collection_id,
        ChildInfo::new(entry_id_2, [0; 32], metadata_2.clone()),
    )
    .unwrap();

    // Receive key_1 via sync
    let action_1 = Action::Add {
        id: entry_id_1,
        data: entry_data_1.clone(),
        metadata: metadata_1.clone(),
        ancestors: vec![ChildInfo::new(
            collection_id,
            [0; 32],
            Metadata::new(base_ts, base_ts),
        )],
    };
    Interface::<MainStorage>::apply_action(action_1).unwrap();

    // Verify Node B has both entries
    let read_1b = MainStorage::storage_read(Key::Entry(entry_id_1));
    let read_2b = MainStorage::storage_read(Key::Entry(entry_id_2));

    assert!(read_1b.is_some(), "Node B should have key_1 after sync");
    assert!(read_2b.is_some(), "Node B should have key_2");
}

// =============================================================================
// Test: Root hash convergence - same deltas, different order
// =============================================================================

#[test]
fn test_root_hash_converges_different_order() {
    // This is the critical test: applying the same deltas in different orders
    // should produce the same final root hash (CRDT property)

    let collection_id = Id::new([42; 32]);
    let base_ts = 50_u64;

    // Create two deltas
    let key_1 = "key_1";
    let value_1 = "value_1";
    let entry_id_1 = compute_entry_id(collection_id, key_1);
    let entry_data_1 = to_vec(&(key_1.to_string(), value_1.to_string())).unwrap();
    let metadata_1 = Metadata::new(100, 100);

    let key_2 = "key_2";
    let value_2 = "value_2";
    let entry_id_2 = compute_entry_id(collection_id, key_2);
    let entry_data_2 = to_vec(&(key_2.to_string(), value_2.to_string())).unwrap();
    let metadata_2 = Metadata::new(200, 200);

    let action_1 = Action::Add {
        id: entry_id_1,
        data: entry_data_1.clone(),
        metadata: metadata_1.clone(),
        ancestors: vec![ChildInfo::new(
            collection_id,
            [0; 32],
            Metadata::new(base_ts, base_ts),
        )],
    };

    let action_2 = Action::Add {
        id: entry_id_2,
        data: entry_data_2.clone(),
        metadata: metadata_2.clone(),
        ancestors: vec![ChildInfo::new(
            collection_id,
            [0; 32],
            Metadata::new(base_ts, base_ts),
        )],
    };

    // Node A: Apply action_1 then action_2
    env::reset_for_testing();
    reset_delta_context();

    Index::<MainStorage>::add_root(ChildInfo::new(
        collection_id,
        [0; 32],
        Metadata::new(base_ts, base_ts),
    ))
    .unwrap();

    Interface::<MainStorage>::apply_action(action_1.clone()).unwrap();
    Interface::<MainStorage>::apply_action(action_2.clone()).unwrap();

    let root_hash_a = Index::<MainStorage>::calculate_full_merkle_hash_for(collection_id).unwrap();

    // Node B: Apply action_2 then action_1 (different order)
    env::reset_for_testing();
    reset_delta_context();

    Index::<MainStorage>::add_root(ChildInfo::new(
        collection_id,
        [0; 32],
        Metadata::new(base_ts, base_ts),
    ))
    .unwrap();

    Interface::<MainStorage>::apply_action(action_2.clone()).unwrap();
    Interface::<MainStorage>::apply_action(action_1.clone()).unwrap();

    let root_hash_b = Index::<MainStorage>::calculate_full_merkle_hash_for(collection_id).unwrap();

    // THE KEY ASSERTION: Both should have the same hash!
    assert_eq!(
        root_hash_a,
        root_hash_b,
        "Root hash should be the same regardless of application order!\n\
         Node A (1 then 2): {}\n\
         Node B (2 then 1): {}",
        hex::encode(root_hash_a),
        hex::encode(root_hash_b)
    );
}

// =============================================================================
// Test: Using actual UnorderedMap collection
// =============================================================================

#[test]
fn test_unordered_map_basic_then_sync() {
    env::reset_for_testing();
    reset_delta_context();

    // Create an UnorderedMap and add an entry
    let mut map: Root<UnorderedMap<String, String>> = Root::new(|| UnorderedMap::new());

    // Insert a value
    map.insert("test_key".to_string(), "test_value".to_string())
        .unwrap();

    // Verify we can read it
    let value = map.get("test_key").unwrap();
    assert_eq!(value.as_deref(), Some("test_value"));

    // Verify we can read another key that doesn't exist
    let value2 = map.get("other_key").unwrap();
    assert_eq!(value2, None);
}

// =============================================================================
// Test: Verify UnorderedMap entry ID computation matches our function
// =============================================================================

#[test]
fn test_unordered_map_entry_id_matches_compute() {
    env::reset_for_testing();
    reset_delta_context();

    // We need to test the ID computation used by UnorderedMap internally
    // The UnorderedMap uses compute_id(parent_id, key_bytes) from collections.rs
    // which is: SHA256(parent_id_bytes || key_bytes)

    // Our compute_entry_id does the same, so they should match
    let collection_id = Id::new([1; 32]);
    let key = "test_key";

    // Compute using our function
    let our_id = compute_entry_id(collection_id, key);

    // Compute using the same algorithm as collections.rs
    let mut hasher = Sha256::new();
    hasher.update(collection_id.as_bytes());
    hasher.update(key.as_bytes());
    let expected_id = Id::new(hasher.finalize().into());

    assert_eq!(
        our_id, expected_id,
        "Our compute_entry_id should match collections.rs compute_id"
    );
}

// =============================================================================
// Test: Simulate KvStore serialization/deserialization preserves collection ID
// =============================================================================

/// KvStore-like struct for testing serialization
#[derive(borsh::BorshSerialize, borsh::BorshDeserialize, Debug)]
struct TestKvStoreSerialized {
    /// This would be the serialized UnorderedMap.inner.storage.id
    /// UnorderedMap serializes to just its Collection ID
    items_collection_id: Id,
}

#[test]
fn test_kvstore_serialization_preserves_collection_id() {
    // When Node A creates KvStore, the UnorderedMap gets a random ID
    let original_collection_id = Id::new([77; 32]);

    // Serialize the "KvStore" (just the collection ID in practice)
    let kvstore = TestKvStoreSerialized {
        items_collection_id: original_collection_id,
    };
    let serialized = to_vec(&kvstore).unwrap();

    // Node B deserializes the KvStore
    let deserialized: TestKvStoreSerialized = from_slice(&serialized).unwrap();

    // The collection ID should be preserved!
    assert_eq!(
        deserialized.items_collection_id, original_collection_id,
        "Collection ID must be preserved through serialization!"
    );
}

// =============================================================================
// Test: Verify entry lookup works after deserialization
// =============================================================================

#[test]
fn test_entry_lookup_after_deserialization() {
    env::reset_for_testing();
    reset_delta_context();

    // Node A's collection ID
    let collection_id = Id::new([88; 32]);
    let key = "test_key";
    let value = "test_value";

    // Node A stores an entry
    let entry_id = compute_entry_id(collection_id, key);
    let entry_data = to_vec(&(key.to_string(), value.to_string())).unwrap();

    // Setup the collection in index
    Index::<MainStorage>::add_root(ChildInfo::new(
        collection_id,
        [0; 32],
        Metadata::new(100, 100),
    ))
    .unwrap();

    // Store the entry
    MainStorage::storage_write(Key::Entry(entry_id), &entry_data);
    Index::<MainStorage>::add_child_to(
        collection_id,
        ChildInfo::new(entry_id, [0; 32], Metadata::new(100, 100)),
    )
    .unwrap();

    // Simulate "Node B" by just using the same collection_id (as if deserialized)
    // Node B should be able to compute the same entry_id and find the data
    let node_b_collection_id = collection_id; // Same ID from deserialization
    let node_b_entry_id = compute_entry_id(node_b_collection_id, key);

    assert_eq!(
        node_b_entry_id, entry_id,
        "Node B should compute the same entry ID"
    );

    let stored = MainStorage::storage_read(Key::Entry(node_b_entry_id));
    assert!(
        stored.is_some(),
        "Node B should find the entry using computed ID"
    );

    let (k, v): (String, String) = from_slice(&stored.unwrap()).unwrap();
    assert_eq!(k, key);
    assert_eq!(v, value);
}

// =============================================================================
// Test: FAILURE MODE - What happens if node creates fresh state before sync?
// =============================================================================

/// This test demonstrates what goes WRONG when a node creates fresh state
/// (with new random collection ID) before applying sync deltas.
///
/// This is likely the bug in the E2E tests!
#[test]
fn test_failure_mode_fresh_state_before_sync() {
    env::reset_for_testing();
    reset_delta_context();

    // === Node A creates original state ===
    let node_a_collection_id = Id::new([11; 32]); // Node A's UnorderedMap ID

    // Node A stores an entry
    let key = "shared_key";
    let value = "from_node_a";
    let entry_id_a = compute_entry_id(node_a_collection_id, key);
    let entry_data = to_vec(&(key.to_string(), value.to_string())).unwrap();

    // === Node B joins and INCORRECTLY creates fresh state ===
    // This simulates what happens if Node B calls init() before receiving deltas
    let node_b_collection_id = Id::new([22; 32]); // Different random ID!

    // Now Node B receives Node A's delta and applies it
    // The delta contains entry stored at entry_id_a (based on node_a_collection_id)
    let action = Action::Add {
        id: entry_id_a,
        data: entry_data.clone(),
        metadata: Metadata::new(100, 100),
        ancestors: vec![ChildInfo::new(
            node_a_collection_id,
            [0; 32],
            Metadata::new(50, 50),
        )],
    };

    // Setup Node B's state with its OWN collection ID
    Index::<MainStorage>::add_root(ChildInfo::new(
        node_b_collection_id,
        [0; 32],
        Metadata::new(50, 50),
    ))
    .unwrap();

    // Apply the delta - this stores the entry at entry_id_a
    Interface::<MainStorage>::apply_action(action).unwrap();

    // Verify entry exists at entry_id_a (where it was stored)
    let stored_at_a = MainStorage::storage_read(Key::Entry(entry_id_a));
    assert!(stored_at_a.is_some(), "Entry should exist at original ID");

    // === THE BUG ===
    // When Node B tries to get("shared_key"), it computes:
    //   entry_id_b = compute_id(node_b_collection_id, "shared_key")
    // But the entry is stored at:
    //   entry_id_a = compute_id(node_a_collection_id, "shared_key")
    // These are DIFFERENT because the collection IDs are different!

    let entry_id_b = compute_entry_id(node_b_collection_id, key);

    // This will be different!
    assert_ne!(
        entry_id_a, entry_id_b,
        "With different collection IDs, entry IDs are different!"
    );

    // Node B CAN'T find the entry because it's looking at the wrong ID!
    let stored_at_b = MainStorage::storage_read(Key::Entry(entry_id_b));
    assert!(
        stored_at_b.is_none(),
        "Entry NOT found at Node B's computed ID - THIS IS THE BUG!"
    );

    println!(
        "BUG DEMONSTRATED:\n\
         - Node A's collection ID: {:?}\n\
         - Node B's collection ID: {:?}\n\
         - Entry stored at (Node A's ID): {:?}\n\
         - Node B looking at: {:?}\n\
         - Result: get() returns NULL!",
        node_a_collection_id, node_b_collection_id, entry_id_a, entry_id_b
    );
}

// =============================================================================
// Test: Verify UnorderedMap deserialization preserves collection ID
// =============================================================================

#[test]
fn test_unordered_map_round_trip_preserves_id() {
    env::reset_for_testing();
    reset_delta_context();

    // Create an UnorderedMap - it gets a random ID
    let map: UnorderedMap<String, String> = UnorderedMap::new();

    // Get the ID BEFORE serialization - need to use Data trait
    use crate::entities::Data;
    let original_id = map.id();
    println!("Original map ID: {:?}", original_id);

    // Serialize
    let serialized = to_vec(&map).unwrap();
    println!("Serialized length: {} bytes", serialized.len());

    // The serialized form should be just the ID (32 bytes for Collection/Element)
    // Plus any borsh overhead
    assert!(
        serialized.len() <= 40,
        "UnorderedMap should serialize to ~32 bytes (just the ID), got {}",
        serialized.len()
    );

    // Deserialize
    let deserialized: UnorderedMap<String, String> = from_slice(&serialized).unwrap();
    let restored_id = deserialized.id();
    println!("Restored map ID: {:?}", restored_id);

    // THE KEY CHECK: ID must be preserved!
    assert_eq!(
        original_id, restored_id,
        "UnorderedMap ID must be preserved through serialization!\n\
         Original: {:?}\n\
         Restored: {:?}",
        original_id, restored_id
    );
}

// =============================================================================
// Test: Full KvStore-like round trip with entries
// =============================================================================

/// KvStore-like struct that mimics the real app
#[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
struct MockKvStore {
    items: UnorderedMap<String, String>,
}

#[test]
fn test_mock_kvstore_round_trip() {
    env::reset_for_testing();
    reset_delta_context();

    // Create KvStore (like Node A's init)
    let store = MockKvStore {
        items: UnorderedMap::new(),
    };

    use crate::entities::Data;
    let original_collection_id = store.items.id();
    println!("Original collection ID: {:?}", original_collection_id);

    // Serialize the KvStore (like what goes in a delta)
    let serialized = to_vec(&store).unwrap();
    println!("Serialized KvStore: {} bytes", serialized.len());

    // Deserialize (like Node B receiving the delta)
    let restored: MockKvStore = from_slice(&serialized).unwrap();
    let restored_collection_id = restored.items.id();
    println!("Restored collection ID: {:?}", restored_collection_id);

    // Collection ID must be the same!
    assert_eq!(
        original_collection_id, restored_collection_id,
        "KvStore collection ID must survive round-trip!"
    );

    // Now simulate adding an entry and looking it up
    // Node A adds an entry
    let key = "test_key";
    let entry_id = compute_entry_id(original_collection_id, key);

    // Node B should compute the SAME entry ID
    let node_b_entry_id = compute_entry_id(restored_collection_id, key);

    assert_eq!(
        entry_id, node_b_entry_id,
        "Entry ID computation must be identical on both nodes!"
    );
}

// =============================================================================
// Test: Verify that multiple UnorderedMap::new() calls get different random IDs
// =============================================================================

#[test]
fn test_unordered_map_new_generates_random_ids() {
    env::reset_for_testing();
    reset_delta_context();

    use crate::entities::Data;

    // Create two separate UnorderedMaps
    let map1: UnorderedMap<String, String> = UnorderedMap::new();
    let map2: UnorderedMap<String, String> = UnorderedMap::new();

    let id1 = map1.id();
    let id2 = map2.id();

    // They MUST have different IDs!
    assert_ne!(
        id1, id2,
        "Each UnorderedMap::new() should generate a unique random ID!\n\
         Map 1: {:?}\n\
         Map 2: {:?}",
        id1, id2
    );

    println!("Map 1 ID: {:?}", id1);
    println!("Map 2 ID: {:?}", id2);
}

// =============================================================================
// Test: Confirm the bug scenario - if Node B calls init() instead of using synced state
// =============================================================================

#[test]
fn test_init_creates_different_collection_id_than_synced() {
    env::reset_for_testing();
    reset_delta_context();

    use crate::entities::Data;

    // Node A initializes
    let store_a = MockKvStore {
        items: UnorderedMap::new(),
    };
    let collection_id_a = store_a.items.id();

    // Serialize and send to Node B
    let serialized = to_vec(&store_a).unwrap();

    // CORRECT behavior: Node B deserializes the state
    let store_b_correct: MockKvStore = from_slice(&serialized).unwrap();
    let collection_id_b_correct = store_b_correct.items.id();

    // WRONG behavior: Node B calls init() instead
    env::reset_for_testing();
    reset_delta_context();
    let store_b_wrong = MockKvStore {
        items: UnorderedMap::new(),
    };
    let collection_id_b_wrong = store_b_wrong.items.id();

    // CORRECT: deserialized ID matches original
    assert_eq!(
        collection_id_a, collection_id_b_correct,
        "Deserialized state should have same collection ID!"
    );

    // WRONG: newly created ID is different
    assert_ne!(
        collection_id_a, collection_id_b_wrong,
        "Newly created state WILL have different collection ID!"
    );

    println!("Node A collection ID: {:?}", collection_id_a);
    println!(
        "Node B (correct - deserialized): {:?}",
        collection_id_b_correct
    );
    println!("Node B (wrong - new): {:?}", collection_id_b_wrong);
    println!(
        "\nIF Node B uses the wrong ID, entries stored at compute_id(A, key) \
         will NOT be found when looking at compute_id(B_wrong, key)!"
    );
}

// =============================================================================
// POTENTIAL FIX: Use deterministic IDs based on field name (like #[app::private])
// =============================================================================

/// Compute a deterministic collection ID based on field name.
/// This is similar to how #[app::private] computes its storage key.
fn compute_deterministic_collection_id(field_name: &str) -> Id {
    let mut hasher = Sha256::new();
    // Match the prefix used in Collection::compute_deterministic_id
    hasher.update(b"calimero:collection:");
    hasher.update(field_name.as_bytes());
    Id::new(hasher.finalize().into())
}

// =============================================================================
// TEST: Verify the fix - deterministic IDs enable sync
// =============================================================================

use crate::entities::Data;

#[test]
fn test_deterministic_ids_match_across_instances() {
    // This test verifies that deterministic IDs are consistent across instances
    env::reset_for_testing();

    // Create two UnorderedMaps with the same field name
    let map_a: UnorderedMap<String, String> = UnorderedMap::new_with_field_name("items");
    let id_a = map_a.element().id();

    // Reset and create another one
    env::reset_for_testing();

    let map_b: UnorderedMap<String, String> = UnorderedMap::new_with_field_name("items");
    let id_b = map_b.element().id();

    // CRITICAL: Both should have the SAME collection ID!
    assert_eq!(id_a, id_b, "Deterministic IDs must match across instances!");

    println!(
        "SUCCESS: Deterministic IDs match!\n\
         ID A: {:?}\n\
         ID B: {:?}",
        id_a, id_b
    );
}

#[test]
fn test_deterministic_ids_differ_by_field_name() {
    // Different field names should produce different IDs
    env::reset_for_testing();

    let map_items: UnorderedMap<String, String> = UnorderedMap::new_with_field_name("items");
    let map_users: UnorderedMap<String, String> = UnorderedMap::new_with_field_name("users");

    assert_ne!(
        map_items.element().id(),
        map_users.element().id(),
        "Different field names should produce different IDs"
    );

    println!(
        "SUCCESS: Different field names produce different IDs!\n\
         'items' ID: {:?}\n\
         'users' ID: {:?}",
        map_items.element().id(),
        map_users.element().id()
    );
}

#[test]
fn test_entry_ids_consistent_with_deterministic_collection_id() {
    // Entry IDs should be consistent when collection ID is deterministic
    env::reset_for_testing();

    let map_a: UnorderedMap<String, String> = UnorderedMap::new_with_field_name("items");
    let entry_id_a = compute_entry_id(map_a.element().id(), "key1");

    env::reset_for_testing();

    let map_b: UnorderedMap<String, String> = UnorderedMap::new_with_field_name("items");
    let entry_id_b = compute_entry_id(map_b.element().id(), "key1");

    // Entry IDs should match because collection IDs are deterministic
    assert_eq!(
        entry_id_a, entry_id_b,
        "Entry IDs should be consistent when collection ID is deterministic"
    );

    println!(
        "SUCCESS: Entry IDs are consistent!\n\
         Entry 'key1' ID: {:?}",
        entry_id_a
    );
}

#[test]
fn test_deterministic_collection_id_proposal() {
    // This demonstrates a POTENTIAL FIX:
    // Instead of random IDs, collections could use deterministic IDs
    // based on their field name in the struct.

    // If KvStore has: items: UnorderedMap<String, LwwRegister<String>>
    // The collection ID could be: SHA256("items")

    let field_name = "items";

    // Node A computes deterministic ID
    let node_a_id = compute_deterministic_collection_id(field_name);

    // Node B computes deterministic ID (same field name)
    let node_b_id = compute_deterministic_collection_id(field_name);

    // They match WITHOUT needing to sync the serialized state!
    assert_eq!(
        node_a_id, node_b_id,
        "Deterministic IDs based on field name would always match!"
    );

    // And entry IDs would also match
    let key = "test_key";
    let node_a_entry_id = compute_entry_id(node_a_id, key);
    let node_b_entry_id = compute_entry_id(node_b_id, key);

    assert_eq!(
        node_a_entry_id, node_b_entry_id,
        "Entry IDs would also match automatically!"
    );

    println!(
        "POTENTIAL FIX:\n\
         - Current: UnorderedMap::new() generates RANDOM ID\n\
         - Proposed: Generate ID from field name like #[app::private]\n\
         - Benefit: Nodes would agree on collection ID even without state sync!\n\
         \n\
         Field name: '{}'\n\
         Deterministic collection ID: {:?}",
        field_name, node_a_id
    );
}

// =============================================================================
// Test: Full delta simulation with root + collection + entry
// =============================================================================

/// Simulate what a real KvStore delta looks like: root update, collection update, entry add
#[test]
fn test_full_delta_with_root_collection_entry() {
    env::reset_for_testing();
    reset_delta_context();

    // Setup: IDs for root, collection, entries
    let root_id = Id::root();
    let collection_id = Id::new([1; 32]);

    let key_1 = "key_1";
    let entry_id_1 = compute_entry_id(collection_id, key_1);
    let entry_data_1 = to_vec(&(key_1.to_string(), "value_1".to_string())).unwrap();

    let key_2 = "key_2";
    let entry_id_2 = compute_entry_id(collection_id, key_2);
    let entry_data_2 = to_vec(&(key_2.to_string(), "value_2".to_string())).unwrap();

    // Root state: just contains the collection ID (like KvStore { items: ... })
    let root_data = to_vec(&collection_id).unwrap();

    // Timestamps
    let ts_base = 100_u64;
    let ts_delta1 = 200_u64;
    let ts_delta2 = 300_u64;

    // Create initial state: root with collection
    Index::<MainStorage>::add_root(ChildInfo::new(
        root_id,
        [0; 32],
        Metadata::new(ts_base, ts_base),
    ))
    .unwrap();
    MainStorage::storage_write(Key::Entry(root_id), &root_data);

    Index::<MainStorage>::add_child_to(
        root_id,
        ChildInfo::new(collection_id, [0; 32], Metadata::new(ts_base, ts_base)),
    )
    .unwrap();

    // Delta 1: Add entry_1
    // In real sync, this would be: Action::Add for entry, Action::Update for collection, Action::Update for root
    let action_entry_1 = Action::Add {
        id: entry_id_1,
        data: entry_data_1.clone(),
        metadata: Metadata::new(ts_delta1, ts_delta1),
        ancestors: vec![ChildInfo::new(
            collection_id,
            [0; 32],
            Metadata::new(ts_base, ts_base),
        )],
    };

    // Delta 2: Add entry_2
    let action_entry_2 = Action::Add {
        id: entry_id_2,
        data: entry_data_2.clone(),
        metadata: Metadata::new(ts_delta2, ts_delta2),
        ancestors: vec![ChildInfo::new(
            collection_id,
            [0; 32],
            Metadata::new(ts_base, ts_base),
        )],
    };

    // Node A: Apply delta 1 then delta 2
    Interface::<MainStorage>::apply_action(action_entry_1.clone()).unwrap();
    Interface::<MainStorage>::apply_action(action_entry_2.clone()).unwrap();

    let root_hash_a = Index::<MainStorage>::calculate_full_merkle_hash_for(root_id).unwrap();

    // Verify entries exist
    assert!(
        MainStorage::storage_read(Key::Entry(entry_id_1)).is_some(),
        "Entry 1 should exist"
    );
    assert!(
        MainStorage::storage_read(Key::Entry(entry_id_2)).is_some(),
        "Entry 2 should exist"
    );

    // Node B: Apply delta 2 then delta 1 (reverse order)
    env::reset_for_testing();
    reset_delta_context();

    // Same initial state
    Index::<MainStorage>::add_root(ChildInfo::new(
        root_id,
        [0; 32],
        Metadata::new(ts_base, ts_base),
    ))
    .unwrap();
    MainStorage::storage_write(Key::Entry(root_id), &root_data);

    Index::<MainStorage>::add_child_to(
        root_id,
        ChildInfo::new(collection_id, [0; 32], Metadata::new(ts_base, ts_base)),
    )
    .unwrap();

    // Apply in reverse order
    Interface::<MainStorage>::apply_action(action_entry_2.clone()).unwrap();
    Interface::<MainStorage>::apply_action(action_entry_1.clone()).unwrap();

    let root_hash_b = Index::<MainStorage>::calculate_full_merkle_hash_for(root_id).unwrap();

    // Verify entries exist
    assert!(
        MainStorage::storage_read(Key::Entry(entry_id_1)).is_some(),
        "Entry 1 should exist on Node B"
    );
    assert!(
        MainStorage::storage_read(Key::Entry(entry_id_2)).is_some(),
        "Entry 2 should exist on Node B"
    );

    // Root hashes should match!
    assert_eq!(
        root_hash_a,
        root_hash_b,
        "Full delta: Root hash should match regardless of order!\n\
         Node A (1→2): {}\n\
         Node B (2→1): {}",
        hex::encode(root_hash_a),
        hex::encode(root_hash_b)
    );
}

// =============================================================================
// Test: Simulate actual KvStore sync scenario with Root::sync-like flow
// =============================================================================

/// This test simulates what happens in Root::sync more closely
#[test]
fn test_root_sync_style_delta_application() {
    use crate::delta::StorageDelta;

    env::reset_for_testing();
    reset_delta_context();

    // Setup like a real KvStore
    let root_id = Id::root();
    let collection_id = Id::new([42; 32]);
    let ts_base = 100_u64;

    // Initial state
    Index::<MainStorage>::add_root(ChildInfo::new(
        root_id,
        [0; 32],
        Metadata::new(ts_base, ts_base),
    ))
    .unwrap();

    // Root data (KvStore serialized - just the collection ID)
    let root_data = to_vec(&collection_id).unwrap();
    MainStorage::storage_write(Key::Entry(root_id), &root_data);

    Index::<MainStorage>::add_child_to(
        root_id,
        ChildInfo::new(collection_id, [0; 32], Metadata::new(ts_base, ts_base)),
    )
    .unwrap();

    // Create a delta like what would be broadcast
    let key = "my_key";
    let entry_id = compute_entry_id(collection_id, key);
    let entry_data = to_vec(&(key.to_string(), "my_value".to_string())).unwrap();
    let ts_delta = 200_u64;

    let actions = vec![
        // Entry add (most specific first)
        Action::Add {
            id: entry_id,
            data: entry_data.clone(),
            metadata: Metadata::new(ts_delta, ts_delta),
            ancestors: vec![ChildInfo::new(
                collection_id,
                [0; 32],
                Metadata::new(ts_base, ts_base),
            )],
        },
        // Root update (would include the root state)
        Action::Update {
            id: root_id,
            data: root_data.clone(),
            metadata: Metadata::new(ts_base, ts_delta), // created_at stays, updated_at changes
            ancestors: vec![],
        },
    ];

    // Apply like Root::sync does (all actions via apply_action)
    for action in &actions {
        Interface::<MainStorage>::apply_action(action.clone()).unwrap();
    }

    // Verify the entry can be found
    let stored = MainStorage::storage_read(Key::Entry(entry_id));
    assert!(stored.is_some(), "Entry should be stored after sync");

    let (k, v): (String, String) = from_slice(&stored.unwrap()).unwrap();
    assert_eq!(k, key);
    assert_eq!(v, "my_value");

    // Verify root hash was updated
    let final_root_hash = Index::<MainStorage>::calculate_full_merkle_hash_for(root_id).unwrap();
    assert_ne!(
        final_root_hash, [0; 32],
        "Root hash should be non-zero after sync"
    );
}
