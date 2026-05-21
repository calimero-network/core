//! Comprehensive tests for CRDT collections
//!
//! Tests all collection types (UnorderedMap, Vector, UnorderedSet)
//! Moved from inline tests in collections modules for better organization

use crate::collections::{Root, UnorderedMap, UnorderedSet, Vector};
use crate::env;
use crate::index::Index;
use crate::store::MainStorage;
use serial_test::serial;

// ============================================================
// Root Tests
// ============================================================

/// `Root<T>` is not a generic LWW register — it is a typed container whose
/// merge semantics are delegated to the application's registered `Mergeable`
/// impl via `merge_root_state` (see `interface::try_merge_data` dispatch on
/// `is_app_root_entry`). So the entry must carry `crdt_type = None`, and
/// HashComparison routes the leaf through `merge_root_state` rather than the
/// generic `apply_lww_winner` path.
///
/// Tagging this entry with an `LwwRegister` `crdt_type` causes silent data
/// loss on cold join: a just-materialised local `Root` whose HLC is *later*
/// than the earlier-written remote `Root` wins the LWW comparison and drops
/// all remote application state. This test pins the contract so a future
/// refactor cannot reintroduce that regression.
#[test]
#[serial]
fn test_root_entry_has_no_crdt_type_so_merge_routes_via_registered_mergeable() {
    // Other tests in this binary also touch `MainStorage` (a global,
    // process-wide store) at the same entry id. Reset so we observe a
    // fresh `Root::new` rather than stale state from a prior test.
    env::reset_for_testing();

    let _root = Root::new(|| UnorderedMap::<String, String>::new());

    // Cross-reference the entry's id by calling `Root::entry_id()` directly
    // instead of hardcoding `[118; 32]`, so this test moves in lock-step with
    // the constructor if that ever changes.
    let entry_id = Root::<UnorderedMap<String, String>>::entry_id();
    let index = <Index<MainStorage>>::get_index(entry_id)
        .expect("get_index should not error")
        .expect("Root entry index should exist");

    // `crdt_type` MUST be `None` — `Root<T>` is dispatched through
    // `merge_root_state` in `interface::try_merge_data`, not the
    // `apply_lww_winner` path. If this regresses to `Some(LwwRegister(...))`,
    // cold-join scenarios with HLC inversion will silently lose data.
    assert_eq!(
        index.metadata.crdt_type, None,
        "Root<T> entry must NOT carry a crdt_type — got {:?}",
        index.metadata.crdt_type
    );

    // `field_name = "root"` is part of the constructor contract — if it
    // regresses, a peer's `compare_tree_nodes` could route the leaf
    // differently. Assert it explicitly so the regression is caught here.
    assert_eq!(
        index.metadata.field_name,
        Some("root".to_string()),
        "Root<T> entry must carry field_name 'root', got {:?}",
        index.metadata.field_name
    );
}

// ============================================================
// UnorderedMap Tests (from collections/unordered_map.rs)
// ============================================================

#[test]
fn test_unordered_map_basic_operations() {
    let mut map = Root::new(|| UnorderedMap::<_, _, MainStorage>::new());

    assert!(map
        .insert("key".to_string(), "value".to_string())
        .expect("insert failed")
        .is_none());

    assert_eq!(
        map.get("key").expect("get failed").as_deref(),
        Some("value")
    );
    assert_ne!(
        map.get("key").expect("get failed").as_deref(),
        Some("value2")
    );

    assert_eq!(
        map.insert("key".to_string(), "value2".to_string())
            .expect("insert failed")
            .as_deref(),
        Some("value")
    );
    assert!(map
        .insert("key2".to_string(), "value".to_string())
        .expect("insert failed")
        .is_none());

    assert_eq!(
        map.get("key").expect("get failed").as_deref(),
        Some("value2")
    );
    assert_eq!(
        map.get("key2").expect("get failed").as_deref(),
        Some("value")
    );

    assert_eq!(
        map.remove("key")
            .expect("error while removing key")
            .as_deref(),
        Some("value2")
    );
    assert_eq!(map.remove("key").expect("error while removing key"), None);

    assert_eq!(map.get("key").expect("get failed"), None);
}

#[test]
fn test_unordered_map_insert_and_get() {
    let mut map = Root::new(|| UnorderedMap::<_, _, MainStorage>::new());

    assert!(map
        .insert("key1".to_string(), "value1".to_string())
        .expect("insert failed")
        .is_none());
    assert!(map
        .insert("key2".to_string(), "value2".to_string())
        .expect("insert failed")
        .is_none());

    assert_eq!(
        map.get("key1").expect("get failed").as_deref(),
        Some("value1")
    );
    assert_eq!(
        map.get("key2").expect("get failed").as_deref(),
        Some("value2")
    );
}

#[test]
fn test_unordered_map_update_value() {
    let mut map = Root::new(|| UnorderedMap::<_, _, MainStorage>::new());

    assert!(map
        .insert("key".to_string(), "value".to_string())
        .expect("insert failed")
        .is_none());
    assert!(!map
        .insert("key".to_string(), "new_value".to_string())
        .expect("insert failed")
        .is_none());

    assert_eq!(
        map.get("key").expect("get failed").as_deref(),
        Some("new_value")
    );
}

#[test]
fn test_unordered_map_remove() {
    let mut map = Root::new(|| UnorderedMap::<_, _, MainStorage>::new());

    assert!(map
        .insert("key".to_string(), "value".to_string())
        .expect("insert failed")
        .is_none());

    assert_eq!(
        map.remove("key").expect("remove failed").as_deref(),
        Some("value")
    );
    assert_eq!(map.get("key").expect("get failed"), None);
}

#[test]
fn test_unordered_map_clear() {
    let mut map = Root::new(|| UnorderedMap::<_, _, MainStorage>::new());

    assert!(map
        .insert("key1".to_string(), "value1".to_string())
        .expect("insert failed")
        .is_none());
    assert!(map
        .insert("key2".to_string(), "value2".to_string())
        .expect("insert failed")
        .is_none());

    map.clear().expect("clear failed");

    assert_eq!(map.get("key1").expect("get failed"), None);
    assert_eq!(map.get("key2").expect("get failed"), None);
}

#[test]
fn test_unordered_map_len() {
    let mut map = Root::new(|| UnorderedMap::<_, _, MainStorage>::new());

    assert_eq!(map.len().expect("len failed"), 0);

    assert!(map
        .insert("key1".to_string(), "value1".to_string())
        .expect("insert failed")
        .is_none());
    assert!(map
        .insert("key2".to_string(), "value2".to_string())
        .expect("insert failed")
        .is_none());
    assert!(!map
        .insert("key2".to_string(), "value3".to_string())
        .expect("insert failed")
        .is_none());

    assert_eq!(map.len().expect("len failed"), 2);

    assert_eq!(
        map.remove("key1").expect("remove failed").as_deref(),
        Some("value1")
    );

    assert_eq!(map.len().expect("len failed"), 1);
}

#[test]
fn test_unordered_map_contains() {
    let mut map = Root::new(|| UnorderedMap::<_, _, MainStorage>::new());

    assert!(map
        .insert("key".to_string(), "value".to_string())
        .expect("insert failed")
        .is_none());

    assert_eq!(map.contains("key").expect("contains failed"), true);
    assert_eq!(map.contains("nonexistent").expect("contains failed"), false);
}

#[test]
fn test_unordered_map_entries() {
    let mut map = Root::new(|| UnorderedMap::<_, _, MainStorage>::new());

    assert!(map
        .insert("key1".to_string(), "value1".to_string())
        .expect("insert failed")
        .is_none());
    assert!(map
        .insert("key2".to_string(), "value2".to_string())
        .expect("insert failed")
        .is_none());
    assert!(!map
        .insert("key2".to_string(), "value3".to_string())
        .expect("insert failed")
        .is_none());

    let entries: Vec<(String, String)> = map.entries().expect("entries failed").collect();

    assert_eq!(entries.len(), 2);
    assert!(entries.contains(&("key1".to_string(), "value1".to_string())));
    assert!(entries.contains(&("key2".to_string(), "value3".to_string())));
}

// ============================================================
// Vector Tests (from collections/vector.rs)
// ============================================================

#[test]
fn test_vector_push() {
    let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

    let value = "test_data".to_string();
    let result = vector.push(value.clone());
    assert!(result.is_ok());
    assert_eq!(vector.len().unwrap(), 1);
}

#[test]
fn test_vector_get() {
    let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

    let value = "test_data".to_string();
    let _ = vector.push(value.clone()).unwrap();
    let retrieved_value = vector.get(0).unwrap();
    assert_eq!(retrieved_value, Some(value));
}

#[test]
fn test_vector_update() {
    let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

    let value1 = "test_data1".to_string();
    let value2 = "test_data2".to_string();
    let _ = vector.push(value1.clone()).unwrap();
    let old = vector.update(0, value2.clone()).unwrap();
    let retrieved_value = vector.get(0).unwrap();
    assert_eq!(retrieved_value, Some(value2));
    assert_eq!(old, Some(value1));
}

#[test]
fn test_vector_get_non_existent() {
    let vector = Root::new(|| Vector::<String>::new());

    match vector.get(0) {
        Ok(retrieved_value) => assert_eq!(retrieved_value, None),
        Err(e) => panic!("Error occurred: {:?}", e),
    }
}

#[test]
fn test_vector_pop() {
    let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

    let value = "test_data".to_string();
    let _ = vector.push(value.clone()).unwrap();
    let popped_value = vector.pop().unwrap();
    assert_eq!(popped_value, Some(value));
    assert_eq!(vector.len().unwrap(), 0);
}

#[test]
fn test_vector_items() {
    let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

    let value1 = "test_data1".to_string();
    let value2 = "test_data2".to_string();
    let _ = vector.push(value1.clone()).unwrap();
    let _ = vector.push(value2.clone()).unwrap();
    let items: Vec<String> = vector.iter().unwrap().collect();
    assert_eq!(items, vec![value1, value2]);
}

#[test]
fn test_vector_contains() {
    let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

    let value = "test_data".to_string();
    let _ = vector.push(value.clone()).unwrap();
    assert!(vector.contains(&value).unwrap());
    let non_existent_value = "non_existent".to_string();
    assert!(!vector.contains(&non_existent_value).unwrap());
}

#[test]
fn test_vector_clear() {
    let mut vector = Root::new(|| Vector::<_, MainStorage>::new());

    let value = "test_data".to_string();
    let _ = vector.push(value.clone()).unwrap();
    vector.clear().unwrap();
    assert_eq!(vector.len().unwrap(), 0);
}

// ============================================================
// UnorderedSet Tests (from collections/unordered_set.rs)
// ============================================================

#[test]
fn test_unordered_set_operations() {
    let mut set = Root::new(|| UnorderedSet::<_, MainStorage>::new());

    assert!(set.insert("value1".to_string()).expect("insert failed"));

    assert_eq!(
        set.contains(&"value1".to_string())
            .expect("contains failed"),
        true
    );

    assert!(!set.insert("value1".to_string()).expect("insert failed"));
    assert!(set.insert("value2".to_string()).expect("insert failed"));

    assert_eq!(set.contains("value3").expect("get failed"), false);
    assert_eq!(set.contains("value2").expect("get failed"), true);

    assert_eq!(
        set.remove("value1").expect("error while removing key"),
        true
    );
    assert_eq!(
        set.remove("value3").expect("error while removing key"),
        false
    );
}

#[test]
fn test_unordered_set_len() {
    let mut set = Root::new(|| UnorderedSet::<_, MainStorage>::new());

    assert!(set.insert("value1".to_string()).expect("insert failed"));
    assert!(set.insert("value2".to_string()).expect("insert failed"));
    assert!(!set.insert("value2".to_string()).expect("insert failed"));

    assert_eq!(set.len().expect("len failed"), 2);

    assert!(set.remove("value1").expect("remove failed"));

    assert_eq!(set.len().expect("len failed"), 1);
}

#[test]
fn test_unordered_set_clear() {
    let mut set = Root::new(|| UnorderedSet::<_, MainStorage>::new());

    assert!(set.insert("value1".to_string()).expect("insert failed"));
    assert!(set.insert("value2".to_string()).expect("insert failed"));

    assert_eq!(set.len().expect("len failed"), 2);

    set.clear().expect("clear failed");

    assert_eq!(set.len().expect("len failed"), 0);
    assert_eq!(set.contains("value1").expect("contains failed"), false);
    assert_eq!(set.contains("value2").expect("contains failed"), false);
}

#[test]
fn test_unordered_set_items() {
    let mut set = Root::new(|| UnorderedSet::<_, MainStorage>::new());

    assert!(set.insert("value1".to_string()).expect("insert failed"));
    assert!(set.insert("value2".to_string()).expect("insert failed"));

    let items: Vec<String> = set.iter().expect("items failed").collect();

    assert_eq!(items.len(), 2);
    assert!(items.contains(&"value1".to_string()));
    assert!(items.contains(&"value2".to_string()));

    assert!(set.remove("value1").expect("remove failed"));
    let items: Vec<String> = set.iter().expect("items failed").collect();
    assert_eq!(items.len(), 1);
}
