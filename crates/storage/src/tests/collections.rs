//! Comprehensive tests for CRDT collections
//!
//! Tests all collection types (UnorderedMap, Vector, UnorderedSet)
//! Moved from inline tests in collections modules for better organization

use crate::collections::{Root, UnorderedMap, UnorderedSet, Vector};

// ============================================================
// UnorderedMap Tests (from collections/unordered_map.rs)
// ============================================================

#[test]
fn test_unordered_map_basic_operations() {
    let mut map = Root::new(|| UnorderedMap::new());

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
    let mut map = Root::new(|| UnorderedMap::new());

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
    let mut map = Root::new(|| UnorderedMap::new());

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
    let mut map = Root::new(|| UnorderedMap::new());

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
    let mut map = Root::new(|| UnorderedMap::new());

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
    let mut map = Root::new(|| UnorderedMap::new());

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
    let mut map = Root::new(|| UnorderedMap::new());

    assert!(map
        .insert("key".to_string(), "value".to_string())
        .expect("insert failed")
        .is_none());

    assert_eq!(map.contains("key").expect("contains failed"), true);
    assert_eq!(map.contains("nonexistent").expect("contains failed"), false);
}

#[test]
fn test_unordered_map_entries() {
    let mut map = Root::new(|| UnorderedMap::new());

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
    let mut vector = Root::new(|| Vector::new());

    let value = "test_data".to_string();
    let result = vector.push(value.clone());
    assert!(result.is_ok());
    assert_eq!(vector.len().unwrap(), 1);
}

#[test]
fn test_vector_get() {
    let mut vector = Root::new(|| Vector::new());

    let value = "test_data".to_string();
    let _ = vector.push(value.clone()).unwrap();
    let retrieved_value = vector.get(0).unwrap();
    assert_eq!(retrieved_value, Some(value));
}

#[test]
fn test_vector_update() {
    let mut vector = Root::new(|| Vector::new());

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
    let mut vector = Root::new(|| Vector::new());

    let value = "test_data".to_string();
    let _ = vector.push(value.clone()).unwrap();
    let popped_value = vector.pop().unwrap();
    assert_eq!(popped_value, Some(value));
    assert_eq!(vector.len().unwrap(), 0);
}

#[test]
fn test_vector_items() {
    let mut vector = Root::new(|| Vector::new());

    let value1 = "test_data1".to_string();
    let value2 = "test_data2".to_string();
    let _ = vector.push(value1.clone()).unwrap();
    let _ = vector.push(value2.clone()).unwrap();
    let items: Vec<String> = vector.iter().unwrap().collect();
    assert_eq!(items, vec![value1, value2]);
}

#[test]
fn test_vector_contains() {
    let mut vector = Root::new(|| Vector::new());

    let value = "test_data".to_string();
    let _ = vector.push(value.clone()).unwrap();
    assert!(vector.contains(&value).unwrap());
    let non_existent_value = "non_existent".to_string();
    assert!(!vector.contains(&non_existent_value).unwrap());
}

#[test]
fn test_vector_clear() {
    let mut vector = Root::new(|| Vector::new());

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
    let mut set = Root::new(|| UnorderedSet::new());

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
    let mut set = Root::new(|| UnorderedSet::new());

    assert!(set.insert("value1".to_string()).expect("insert failed"));
    assert!(set.insert("value2".to_string()).expect("insert failed"));
    assert!(!set.insert("value2".to_string()).expect("insert failed"));

    assert_eq!(set.len().expect("len failed"), 2);

    assert!(set.remove("value1").expect("remove failed"));

    assert_eq!(set.len().expect("len failed"), 1);
}

#[test]
fn test_unordered_set_clear() {
    let mut set = Root::new(|| UnorderedSet::new());

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
    let mut set = Root::new(|| UnorderedSet::new());

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
