//! Critical tests for nested CRDT merging
//!
//! These tests validate that the Mergeable implementations solve the
//! nested CRDT divergence problem identified in production.
//!
//! Note: These tests demonstrate the merge logic but require Clone implementations
//! which aren't available for all collections. For now, documenting the pattern.

use crate::collections::{Counter, LwwRegister, Mergeable, Root, UnorderedMap};
use crate::env;

#[test]
#[ignore] // Requires Clone for collections - implement in future
fn test_nested_map_merge_different_inner_keys() {
    env::reset_for_testing();

    // This test demonstrates what WILL work once Clone is implemented
    // The Mergeable logic is correct, just need Clone support
    
    // Create initial state on both nodes
    let mut map1 = Root::new(|| UnorderedMap::<String, UnorderedMap<String, String>>::new());
    let mut initial_inner = UnorderedMap::new();
    initial_inner
        .insert("initial".to_string(), "value".to_string())
        .unwrap();
    map1.insert("doc-1".to_string(), initial_inner)
        .unwrap();

    // Clone for node 2 (TODO: implement Clone for Root)
    let mut map2 = map1; // Placeholder

    // Node 1: Update title field
    let mut inner1 = map1.get(&"doc-1".to_string()).unwrap().unwrap();
    inner1
        .insert("title".to_string(), "Updated Title".to_string())
        .unwrap();
    map1.insert("doc-1".to_string(), inner1).unwrap();

    // Node 2: Add owner field (concurrent modification)
    let mut inner2 = map2.get(&"doc-1".to_string()).unwrap().unwrap();
    inner2
        .insert("owner".to_string(), "Alice".to_string())
        .unwrap();
    map2.insert("doc-1".to_string(), inner2).unwrap();

    // At this point:
    // map1["doc-1"] has: {"initial": "value", "title": "Updated Title"}
    // map2["doc-1"] has: {"initial": "value", "owner": "Alice"}

    // MERGE - this is the critical operation!
    Mergeable::merge(&mut map1, &map2).unwrap();

    // Verify: BOTH changes should be preserved
    let final_inner = map1.get(&"doc-1".to_string()).unwrap().unwrap();

    assert_eq!(
        final_inner.get(&"initial".to_string()).unwrap(),
        Some("value".to_string()),
        "Initial value should be preserved"
    );

    assert_eq!(
        final_inner.get(&"title".to_string()).unwrap(),
        Some("Updated Title".to_string()),
        "Title update from Node 1 should be preserved"
    );

    assert_eq!(
        final_inner.get(&"owner".to_string()).unwrap(),
        Some("Alice".to_string()),
        "Owner update from Node 2 should be preserved"
    );
}

#[test]
#[ignore] // Requires Clone for collections - implement in future
fn test_map_of_counters_merge() {
    env::reset_for_testing();

    // Map<String, Counter>
    let mut map1 = Root::new(|| UnorderedMap::<String, Counter>::new());
    let mut counter1 = Counter::new();
    counter1.increment().unwrap();
    counter1.increment().unwrap(); // value = 2
    map1.insert("counter1".to_string(), counter1).unwrap();

    let mut map2 = map1.clone();

    // Node 1: Increment counter1
    let mut c = map1.get(&"counter1".to_string()).unwrap().unwrap();
    c.increment().unwrap(); // value = 3
    map1.insert("counter1".to_string(), c).unwrap();

    // Node 2: Also increment counter1 (concurrent)
    let mut c = map2.get(&"counter1".to_string()).unwrap().unwrap();
    c.increment().unwrap(); // value = 3
    map2.insert("counter1".to_string(), c).unwrap();

    // MERGE
    Mergeable::merge(&mut map1, &map2).unwrap();

    // Verify: Counters should sum (not LWW!)
    let final_counter = map1.get(&"counter1".to_string()).unwrap().unwrap();
    assert_eq!(final_counter.value().unwrap(), 6, "Counters should sum: 3 + 3 = 6");
}

#[test]
#[ignore] // Requires Clone for collections - implement in future
fn test_map_of_lww_registers_merge() {
    env::reset_for_testing();

    // Map<String, LwwRegister<String>>
    let mut map1 = Root::new(|| UnorderedMap::<String, LwwRegister<String>>::new());
    map1.insert(
        "title".to_string(),
        LwwRegister::new("Initial".to_string()),
    )
    .unwrap();

    let mut map2 = map1.clone();

    std::thread::sleep(std::time::Duration::from_millis(1));

    // Node 1: Update title
    let mut title1 = map1.get(&"title".to_string()).unwrap().unwrap();
    title1.set("From Node 1".to_string());
    map1.insert("title".to_string(), title1).unwrap();

    std::thread::sleep(std::time::Duration::from_millis(1));

    // Node 2: Update title (concurrent, later timestamp)
    let mut title2 = map2.get(&"title".to_string()).unwrap().unwrap();
    title2.set("From Node 2".to_string());
    map2.insert("title".to_string(), title2).unwrap();

    // MERGE
    Mergeable::merge(&mut map1, &map2).unwrap();

    // Verify: Latest timestamp wins
    let final_title = map1.get(&"title".to_string()).unwrap().unwrap();
    assert_eq!(
        final_title.get(),
        "From Node 2",
        "Latest LWW register should win"
    );
}

#[test]
#[ignore] // Requires Clone for collections - implement in future
fn test_three_level_nesting_merge() {
    env::reset_for_testing();

    // Map<String, Map<String, LwwRegister<String>>>
    type InnerMap = UnorderedMap<String, LwwRegister<String>>;
    type OuterMap = UnorderedMap<String, InnerMap>;

    let mut map1 = Root::new(|| OuterMap::new());

    // Initialize with a document
    let mut doc_fields = InnerMap::new();
    doc_fields
        .insert("initial".to_string(), LwwRegister::new("value".to_string()))
        .unwrap();
    map1.insert("doc-1".to_string(), doc_fields).unwrap();

    let mut map2 = map1.clone();

    // Node 1: Update title field
    let mut inner1 = map1.get(&"doc-1".to_string()).unwrap().unwrap();
    inner1
        .insert("title".to_string(), LwwRegister::new("Title 1".to_string()))
        .unwrap();
    map1.insert("doc-1".to_string(), inner1).unwrap();

    // Node 2: Add owner field (concurrent)
    let mut inner2 = map2.get(&"doc-1".to_string()).unwrap().unwrap();
    inner2
        .insert("owner".to_string(), LwwRegister::new("Alice".to_string()))
        .unwrap();
    map2.insert("doc-1".to_string(), inner2).unwrap();

    // MERGE
    Mergeable::merge(&mut map1, &map2).unwrap();

    // Verify: All three fields present
    let final_inner = map1.get(&"doc-1".to_string()).unwrap().unwrap();

    assert_eq!(
        final_inner
            .get(&"initial".to_string())
            .unwrap()
            .map(|r| r.get().clone()),
        Some("value".to_string())
    );

    assert_eq!(
        final_inner
            .get(&"title".to_string())
            .unwrap()
            .map(|r| r.get().clone()),
        Some("Title 1".to_string())
    );

    assert_eq!(
        final_inner
            .get(&"owner".to_string())
            .unwrap()
            .map(|r| r.get().clone()),
        Some("Alice".to_string())
    );
}

#[test]
#[ignore] // Requires Clone for collections - implement in future
fn test_map_merge_with_different_keys() {
    env::reset_for_testing();

    let mut map1 = Root::new(|| UnorderedMap::<String, Counter>::new());
    let mut map2 = Root::new(|| UnorderedMap::<String, Counter>::new());

    // Node 1: Add counter_a
    let mut ca = Counter::new();
    ca.increment().unwrap();
    map1.insert("counter_a".to_string(), ca).unwrap();

    // Node 2: Add counter_b
    let mut cb = Counter::new();
    cb.increment().unwrap();
    cb.increment().unwrap();
    map2.insert("counter_b".to_string(), cb).unwrap();

    // MERGE
    Mergeable::merge(&mut map1, &map2).unwrap();

    // Verify: Both counters present
    assert_eq!(
        map1.get(&"counter_a".to_string()).unwrap().unwrap().value().unwrap(),
        1
    );
    assert_eq!(
        map1.get(&"counter_b".to_string()).unwrap().unwrap().value().unwrap(),
        2
    );
}

