//! Tests for concurrent merge scenarios
//!
//! These tests simulate the exact scenario that happens during E2E sync:
//! Two nodes make concurrent updates, and we verify that merge works correctly.

#![allow(unused_results)]

use crate::collections::{LwwRegister, Mergeable, UnorderedMap};
use crate::env;
use crate::merge::{merge_root_state, MergeRegistry};
use borsh::{BorshDeserialize, BorshSerialize};

// ============================================================================
// Test Types - Simulating KvStore without storage layer
// ============================================================================

/// Pure KvStore simulation - no storage operations
#[derive(BorshSerialize, BorshDeserialize, Debug, Clone, PartialEq)]
struct PureKvStore {
    /// Simulates UnorderedMap<String, LwwRegister<String>>
    /// Using BTreeMap for deterministic ordering in tests
    items: std::collections::BTreeMap<String, PureLwwValue>,
}

/// Pure LWW value without storage
#[derive(BorshSerialize, BorshDeserialize, Debug, Clone, PartialEq)]
struct PureLwwValue {
    value: String,
    timestamp: u64,
}

impl PureLwwValue {
    fn new(value: String, timestamp: u64) -> Self {
        Self { value, timestamp }
    }

    fn merge(&mut self, other: &Self) {
        // Last-Write-Wins by timestamp
        if other.timestamp > self.timestamp {
            self.value = other.value.clone();
            self.timestamp = other.timestamp;
        }
    }
}

impl Mergeable for PureKvStore {
    fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
        // Merge all entries from other
        for (key, other_value) in &other.items {
            if let Some(our_value) = self.items.get_mut(key) {
                // Key exists in both - LWW merge
                our_value.merge(other_value);
            } else {
                // Key only in other - add it
                self.items.insert(key.clone(), other_value.clone());
            }
        }
        Ok(())
    }
}

impl PureKvStore {
    fn new() -> Self {
        Self {
            items: std::collections::BTreeMap::new(),
        }
    }

    fn set(&mut self, key: String, value: String, timestamp: u64) {
        self.items.insert(key, PureLwwValue::new(value, timestamp));
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.items.get(key).map(|v| v.value.as_str())
    }

    fn keys(&self) -> Vec<&str> {
        self.items.keys().map(|s| s.as_str()).collect()
    }
}

// ============================================================================
// Unit Tests
// ============================================================================

#[test]
fn test_pure_kv_merge_disjoint_keys() {
    // Scenario: Two nodes write different keys concurrently
    let mut store1 = PureKvStore::new();
    store1.set("key_1".to_string(), "from_node1".to_string(), 100);

    let mut store2 = PureKvStore::new();
    store2.set("key_2".to_string(), "from_node2".to_string(), 200);

    // Merge store2 into store1
    store1.merge(&store2).unwrap();

    // Both keys should exist
    assert_eq!(store1.get("key_1"), Some("from_node1"));
    assert_eq!(store1.get("key_2"), Some("from_node2"));
    assert_eq!(store1.keys().len(), 2);
}

#[test]
fn test_pure_kv_merge_same_key_lww() {
    // Scenario: Both nodes write the same key, LWW should resolve
    let mut store1 = PureKvStore::new();
    store1.set("shared_key".to_string(), "old_value".to_string(), 100);

    let mut store2 = PureKvStore::new();
    store2.set("shared_key".to_string(), "new_value".to_string(), 200);

    // Merge store2 into store1 - store2 has newer timestamp
    store1.merge(&store2).unwrap();

    assert_eq!(store1.get("shared_key"), Some("new_value"));
}

#[test]
fn test_pure_kv_merge_same_key_older_timestamp() {
    // Scenario: Incoming has older timestamp - should NOT overwrite
    let mut store1 = PureKvStore::new();
    store1.set("shared_key".to_string(), "newer_value".to_string(), 200);

    let mut store2 = PureKvStore::new();
    store2.set("shared_key".to_string(), "older_value".to_string(), 100);

    // Merge store2 into store1 - store1 is newer, should keep
    store1.merge(&store2).unwrap();

    assert_eq!(store1.get("shared_key"), Some("newer_value"));
}

#[test]
fn test_pure_kv_merge_concurrent_10_keys_each() {
    // Scenario: Simulates the E2E test - each node writes 10 unique keys
    let mut store1 = PureKvStore::new();
    for i in 0..10 {
        store1.set(
            format!("key_1_{}", i),
            format!("value_from_node1_{}", i),
            100 + i as u64,
        );
    }

    let mut store2 = PureKvStore::new();
    for i in 0..10 {
        store2.set(
            format!("key_2_{}", i),
            format!("value_from_node2_{}", i),
            200 + i as u64,
        );
    }

    // Merge store2 into store1
    store1.merge(&store2).unwrap();

    // All 20 keys should exist
    assert_eq!(
        store1.keys().len(),
        20,
        "Should have all 20 keys after merge"
    );

    // Verify all keys from both nodes
    for i in 0..10 {
        assert_eq!(
            store1.get(&format!("key_1_{}", i)),
            Some(format!("value_from_node1_{}", i).as_str()),
            "Missing key_1_{} from node1",
            i
        );
        assert_eq!(
            store1.get(&format!("key_2_{}", i)),
            Some(format!("value_from_node2_{}", i).as_str()),
            "Missing key_2_{} from node2",
            i
        );
    }
}

#[test]
fn test_merge_via_borsh_serialization() {
    // Test the actual borsh serialization round-trip used in merge_root_state
    let mut store1 = PureKvStore::new();
    store1.set("key_1".to_string(), "from_node1".to_string(), 100);

    let mut store2 = PureKvStore::new();
    store2.set("key_2".to_string(), "from_node2".to_string(), 200);

    // Serialize both
    let bytes1 = borsh::to_vec(&store1).unwrap();
    let bytes2 = borsh::to_vec(&store2).unwrap();

    // Deserialize store1
    let mut merged: PureKvStore = borsh::from_slice(&bytes1).unwrap();
    // Deserialize store2
    let other: PureKvStore = borsh::from_slice(&bytes2).unwrap();
    // Merge
    merged.merge(&other).unwrap();

    // Verify
    assert_eq!(merged.get("key_1"), Some("from_node1"));
    assert_eq!(merged.get("key_2"), Some("from_node2"));
}

#[test]
fn test_merge_root_state_with_injectable_registry() {
    // Test using the injectable MergeRegistry
    let mut registry = MergeRegistry::new();
    registry.register::<PureKvStore>();

    let mut store1 = PureKvStore::new();
    store1.set("key_1".to_string(), "from_node1".to_string(), 100);

    let mut store2 = PureKvStore::new();
    store2.set("key_2".to_string(), "from_node2".to_string(), 200);

    let bytes1 = borsh::to_vec(&store1).unwrap();
    let bytes2 = borsh::to_vec(&store2).unwrap();

    // Use registry to merge (try_merge tries all registered functions)
    let result = registry.try_merge(&bytes1, &bytes2, 100, 200);
    assert!(result.is_some(), "Merge function should be found");

    let merged_bytes = result.unwrap().expect("Merge should succeed");
    let merged: PureKvStore = borsh::from_slice(&merged_bytes).unwrap();

    assert_eq!(merged.get("key_1"), Some("from_node1"));
    assert_eq!(merged.get("key_2"), Some("from_node2"));
}

#[test]
fn test_merge_symmetry() {
    // Verify merge(A, B) produces same result as merge(B, A)
    // (Commutativity for disjoint keys)
    let mut store1 = PureKvStore::new();
    store1.set("key_1".to_string(), "value1".to_string(), 100);

    let mut store2 = PureKvStore::new();
    store2.set("key_2".to_string(), "value2".to_string(), 200);

    // Merge A into B
    let mut result_ab = store1.clone();
    result_ab.merge(&store2).unwrap();

    // Merge B into A
    let mut result_ba = store2.clone();
    result_ba.merge(&store1).unwrap();

    // Results should be equivalent
    assert_eq!(result_ab.keys().len(), result_ba.keys().len());
    assert_eq!(result_ab.get("key_1"), result_ba.get("key_1"));
    assert_eq!(result_ab.get("key_2"), result_ba.get("key_2"));
}

// ============================================================================
// Tests with actual storage types (slower, but test real implementation)
// ============================================================================

#[test]
fn test_real_unordered_map_merge() {
    env::reset_for_testing();

    // Create two maps with disjoint keys
    let mut map1: UnorderedMap<String, LwwRegister<String>> = UnorderedMap::new();
    map1.insert("key_1".to_string(), LwwRegister::new("value1".to_string()))
        .unwrap();

    let mut map2: UnorderedMap<String, LwwRegister<String>> = UnorderedMap::new();
    map2.insert("key_2".to_string(), LwwRegister::new("value2".to_string()))
        .unwrap();

    // Merge map2 into map1
    map1.merge(&map2).unwrap();

    // Verify both keys exist
    let entries: Vec<_> = map1.entries().unwrap().collect();
    assert_eq!(entries.len(), 2, "Should have 2 entries after merge");

    // Check specific keys
    assert!(
        map1.get(&"key_1".to_string()).unwrap().is_some(),
        "Should have key_1"
    );
    assert!(
        map1.get(&"key_2".to_string()).unwrap().is_some(),
        "Should have key_2"
    );
}

#[test]
fn test_real_unordered_map_merge_10_keys() {
    env::reset_for_testing();

    // Simulate the E2E scenario with real types
    let mut map1: UnorderedMap<String, LwwRegister<String>> = UnorderedMap::new();
    for i in 0..10 {
        map1.insert(
            format!("key_1_{}", i),
            LwwRegister::new(format!("value_from_node1_{}", i)),
        )
        .unwrap();
    }

    let mut map2: UnorderedMap<String, LwwRegister<String>> = UnorderedMap::new();
    for i in 0..10 {
        map2.insert(
            format!("key_2_{}", i),
            LwwRegister::new(format!("value_from_node2_{}", i)),
        )
        .unwrap();
    }

    // Merge map2 into map1
    map1.merge(&map2).unwrap();

    // Should have all 20 keys
    let entries: Vec<_> = map1.entries().unwrap().collect();
    assert_eq!(
        entries.len(),
        20,
        "Should have 20 entries after merge, got {}",
        entries.len()
    );

    // Verify specific keys exist
    for i in 0..10 {
        assert!(
            map1.get(&format!("key_1_{}", i)).unwrap().is_some(),
            "Missing key_1_{} from node1",
            i
        );
        assert!(
            map1.get(&format!("key_2_{}", i)).unwrap().is_some(),
            "Missing key_2_{} from node2",
            i
        );
    }
}

// ============================================================================
// Integration test: Full merge_root_state flow
// ============================================================================

#[test]
#[serial_test::serial]
fn test_global_registry_merge() {
    use crate::merge::{clear_merge_registry, register_crdt_merge};

    // Note: This test uses global state, so needs serial
    env::reset_for_testing();

    // Register PureKvStore (simulates what #[app::state] does)
    register_crdt_merge::<PureKvStore>();

    let mut store1 = PureKvStore::new();
    store1.set("node1_key".to_string(), "node1_value".to_string(), 100);

    let mut store2 = PureKvStore::new();
    store2.set("node2_key".to_string(), "node2_value".to_string(), 200);

    let bytes1 = borsh::to_vec(&store1).unwrap();
    let bytes2 = borsh::to_vec(&store2).unwrap();

    // This is what save_internal calls
    let merged_bytes = merge_root_state(&bytes1, &bytes2, 100, 200).unwrap();

    let merged: PureKvStore = borsh::from_slice(&merged_bytes).unwrap();
    assert_eq!(merged.get("node1_key"), Some("node1_value"));
    assert_eq!(merged.get("node2_key"), Some("node2_value"));

    clear_merge_registry();
}

// ============================================================================
// Tests for save_internal merge path
// ============================================================================

/// Test that when incoming timestamp is OLDER, merge still happens for root
/// (This was the bug - LWW was rejecting older timestamps before merge)
#[test]
#[serial_test::serial]
fn test_merge_root_older_incoming_timestamp() {
    use crate::merge::{clear_merge_registry, register_crdt_merge};

    env::reset_for_testing();
    register_crdt_merge::<PureKvStore>();

    // Existing state (newer timestamp)
    let mut existing = PureKvStore::new();
    existing.set(
        "existing_key".to_string(),
        "existing_value".to_string(),
        200,
    );

    // Incoming state (older timestamp)
    let mut incoming = PureKvStore::new();
    incoming.set(
        "incoming_key".to_string(),
        "incoming_value".to_string(),
        100,
    );

    let bytes_existing = borsh::to_vec(&existing).unwrap();
    let bytes_incoming = borsh::to_vec(&incoming).unwrap();

    // Merge should still combine both, even though incoming is "older"
    let merged_bytes = merge_root_state(&bytes_existing, &bytes_incoming, 200, 100).unwrap();

    let merged: PureKvStore = borsh::from_slice(&merged_bytes).unwrap();

    // KEY ASSERTION: Both keys should exist!
    // The old bug was rejecting incoming entirely due to older timestamp
    assert_eq!(
        merged.get("existing_key"),
        Some("existing_value"),
        "Should keep existing key"
    );
    assert_eq!(
        merged.get("incoming_key"),
        Some("incoming_value"),
        "Should add incoming key even with older timestamp"
    );

    clear_merge_registry();
}

/// Test LWW behavior when same key exists in both states
#[test]
#[serial_test::serial]
fn test_merge_root_same_key_lww() {
    use crate::merge::{clear_merge_registry, register_crdt_merge};

    env::reset_for_testing();
    register_crdt_merge::<PureKvStore>();

    // Existing state
    let mut existing = PureKvStore::new();
    existing.set("shared_key".to_string(), "old_value".to_string(), 100);

    // Incoming state (newer)
    let mut incoming = PureKvStore::new();
    incoming.set("shared_key".to_string(), "new_value".to_string(), 200);

    let bytes_existing = borsh::to_vec(&existing).unwrap();
    let bytes_incoming = borsh::to_vec(&incoming).unwrap();

    let merged_bytes = merge_root_state(&bytes_existing, &bytes_incoming, 100, 200).unwrap();
    let merged: PureKvStore = borsh::from_slice(&merged_bytes).unwrap();

    // LWW: newer value should win
    assert_eq!(merged.get("shared_key"), Some("new_value"));

    clear_merge_registry();
}

/// Test that merge is idempotent (merging same data multiple times)
#[test]
#[serial_test::serial]
fn test_merge_idempotent() {
    use crate::merge::{clear_merge_registry, register_crdt_merge};

    env::reset_for_testing();
    register_crdt_merge::<PureKvStore>();

    let mut store = PureKvStore::new();
    store.set("key".to_string(), "value".to_string(), 100);

    let bytes = borsh::to_vec(&store).unwrap();

    // Merge with itself
    let merged_bytes = merge_root_state(&bytes, &bytes, 100, 100).unwrap();
    let merged: PureKvStore = borsh::from_slice(&merged_bytes).unwrap();

    assert_eq!(merged.keys().len(), 1);
    assert_eq!(merged.get("key"), Some("value"));

    // Merge again
    let merged_bytes2 = merge_root_state(&merged_bytes, &bytes, 100, 100).unwrap();
    let merged2: PureKvStore = borsh::from_slice(&merged_bytes2).unwrap();

    assert_eq!(merged2.keys().len(), 1);
    assert_eq!(merged2.get("key"), Some("value"));

    clear_merge_registry();
}

/// Test that unregistered type falls back to LWW (not merge)
#[test]
#[serial_test::serial]
fn test_unregistered_type_fallback_lww() {
    use crate::merge::clear_merge_registry;

    env::reset_for_testing();
    clear_merge_registry(); // Ensure no types registered

    // Two different states
    let mut store1 = PureKvStore::new();
    store1.set("key1".to_string(), "value1".to_string(), 100);

    let mut store2 = PureKvStore::new();
    store2.set("key2".to_string(), "value2".to_string(), 200);

    let bytes1 = borsh::to_vec(&store1).unwrap();
    let bytes2 = borsh::to_vec(&store2).unwrap();

    // Without registration, merge_root_state should fallback to LWW
    // LWW picks the one with newer timestamp (store2, ts=200)
    let merged_bytes = merge_root_state(&bytes1, &bytes2, 100, 200).unwrap();
    let merged: PureKvStore = borsh::from_slice(&merged_bytes).unwrap();

    // LWW fallback: incoming is newer, so only key2 should exist
    assert!(
        merged.get("key1").is_none(),
        "LWW fallback should NOT merge, only keep newer"
    );
    assert_eq!(
        merged.get("key2"),
        Some("value2"),
        "LWW fallback should keep newer state"
    );

    clear_merge_registry();
}

// ============================================================================
// Tests for try_merge_data function (what save_internal calls)
// ============================================================================

/// Test that try_merge_data delegates to merge_root_state correctly
#[test]
#[serial_test::serial]
fn test_try_merge_data_delegates_correctly() {
    use crate::merge::{clear_merge_registry, merge_root_state, register_crdt_merge};

    env::reset_for_testing();
    clear_merge_registry();
    register_crdt_merge::<PureKvStore>();

    // Create two stores with different keys
    let mut store1 = PureKvStore::new();
    store1.set("key1".to_string(), "value1".to_string(), 100);

    let mut store2 = PureKvStore::new();
    store2.set("key2".to_string(), "value2".to_string(), 200);

    let bytes1 = borsh::to_vec(&store1).unwrap();
    let bytes2 = borsh::to_vec(&store2).unwrap();

    // This is exactly what Interface::try_merge_data calls
    let merged = merge_root_state(&bytes1, &bytes2, 100, 200).unwrap();

    let result: PureKvStore = borsh::from_slice(&merged).unwrap();
    assert_eq!(result.get("key1"), Some("value1"), "Should have key1");
    assert_eq!(result.get("key2"), Some("value2"), "Should have key2");

    clear_merge_registry();
}

/// Test merge behavior when existing is newer (important for the bug!)
#[test]
#[serial_test::serial]
fn test_try_merge_data_existing_newer() {
    use crate::merge::{clear_merge_registry, merge_root_state, register_crdt_merge};

    env::reset_for_testing();
    clear_merge_registry();
    register_crdt_merge::<PureKvStore>();

    // Existing state is "newer" (higher timestamp)
    let mut existing = PureKvStore::new();
    existing.set(
        "existing_key".to_string(),
        "existing_value".to_string(),
        200,
    );

    // Incoming state is "older" (lower timestamp)
    let mut incoming = PureKvStore::new();
    incoming.set(
        "incoming_key".to_string(),
        "incoming_value".to_string(),
        100,
    );

    let bytes_existing = borsh::to_vec(&existing).unwrap();
    let bytes_incoming = borsh::to_vec(&incoming).unwrap();

    // Merge: existing has ts=200, incoming has ts=100
    // This is the key scenario - older incoming should still be merged!
    let merged = merge_root_state(&bytes_existing, &bytes_incoming, 200, 100).unwrap();

    let result: PureKvStore = borsh::from_slice(&merged).unwrap();

    // KEY ASSERTION: Both keys should exist!
    // The bug was LWW rejecting the entire incoming state because ts=100 < ts=200
    assert_eq!(
        result.get("existing_key"),
        Some("existing_value"),
        "Must keep existing key"
    );
    assert_eq!(
        result.get("incoming_key"),
        Some("incoming_value"),
        "Must merge incoming key even with older timestamp"
    );

    clear_merge_registry();
}

/// Test the full scenario: 10 keys each, concurrent merge
#[test]
#[serial_test::serial]
fn test_concurrent_10_keys_each_via_merge_root_state() {
    use crate::merge::{clear_merge_registry, merge_root_state, register_crdt_merge};

    env::reset_for_testing();
    clear_merge_registry();
    register_crdt_merge::<PureKvStore>();

    // Simulate Node 1 state
    let mut node1 = PureKvStore::new();
    for i in 0..10 {
        node1.set(
            format!("key_1_{}", i),
            format!("value_from_node1_{}", i),
            100 + i as u64,
        );
    }

    // Simulate Node 2 state
    let mut node2 = PureKvStore::new();
    for i in 0..10 {
        node2.set(
            format!("key_2_{}", i),
            format!("value_from_node2_{}", i),
            200 + i as u64,
        );
    }

    let bytes1 = borsh::to_vec(&node1).unwrap();
    let bytes2 = borsh::to_vec(&node2).unwrap();

    // Merge from Node 1's perspective (receiving Node 2's state)
    let merged = merge_root_state(&bytes1, &bytes2, 100, 200).unwrap();
    let result: PureKvStore = borsh::from_slice(&merged).unwrap();

    // Should have all 20 keys
    assert_eq!(result.keys().len(), 20, "Should have all 20 keys");

    // Verify all keys exist
    for i in 0..10 {
        assert!(
            result.get(&format!("key_1_{}", i)).is_some(),
            "Missing key_1_{} from node1",
            i
        );
        assert!(
            result.get(&format!("key_2_{}", i)).is_some(),
            "Missing key_2_{} from node2",
            i
        );
    }

    clear_merge_registry();
}
