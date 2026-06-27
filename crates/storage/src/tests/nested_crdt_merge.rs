//! Critical tests for nested CRDT merging
//!
//! These tests validate that the Mergeable implementations solve the
//! nested CRDT divergence problem identified in production.
//!
//! Pattern: two independent nodes build their state, then merge. No Clone needed —
//! each "node" is modelled as a separate collection instance. Different executor IDs
//! ensure Counter increments are tracked independently per node.
//!
//! Test isolation contract: every test calls `env::reset_for_testing()` as its first
//! statement. `reset_for_testing()` calls `reset_environment()`, which unconditionally
//! resets the executor ID to `[237;32]` (the default), clears mock storage, and resets
//! the HLC. This means any state mutated by a prior test — including a leaked executor
//! ID from a panicking test — is fully cleared before the current test's body runs.
//! The `#[serial]` attribute ensures tests run sequentially, so no concurrent state
//! corruption is possible. Tests that call `env::set_executor_id` do NOT need a trailing
//! reset: the next test's leading `reset_for_testing()` is the correct and sufficient
//! cleanup point.

use core::num::NonZeroU128;

use serial_test::serial;

use crate::collections::{Counter, LwwRegister, Mergeable, UnorderedMap};
use crate::env;
use crate::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};

/// Build a HybridTimestamp with an explicit logical time and node ID.
///
/// `node` distinguishes the originating replica for tiebreaking when two
/// timestamps share the same `NTP64` value.  Pass distinct values for each
/// simulated node so tiebreak logic is exercised correctly.
///
/// The parameter is `NonZeroU128` rather than `u128` so that passing `0`
/// is a compile-time error instead of a runtime panic.
fn ts(time: u64, node: NonZeroU128) -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(NTP64(time), ID::from(node)))
}

#[test]
#[serial]
fn test_nested_map_merge_different_inner_keys() {
    env::reset_for_testing();

    // This test covers disjoint-key divergence: node 1 and node 2 each add a
    // unique key to the same outer-map entry, and the merge must union them.
    // Same-key LWW conflict resolution (where both nodes write the same key
    // with different values) is covered by test_map_of_lww_registers_merge.

    // Node 1: doc-1 has "initial" + "title"
    let mut map1 = UnorderedMap::<String, UnorderedMap<String, LwwRegister<String>>>::new();
    let mut inner1 = UnorderedMap::new();
    inner1
        .insert("initial".to_string(), LwwRegister::new("value".to_string()))
        .unwrap();
    inner1
        .insert(
            "title".to_string(),
            LwwRegister::new("Updated Title".to_string()),
        )
        .unwrap();
    map1.insert("doc-1".to_string(), inner1).unwrap();

    // Node 2: doc-1 has "initial" + "owner" (concurrent modification)
    let mut map2 = UnorderedMap::<String, UnorderedMap<String, LwwRegister<String>>>::new();
    let mut inner2 = UnorderedMap::new();
    inner2
        .insert("initial".to_string(), LwwRegister::new("value".to_string()))
        .unwrap();
    inner2
        .insert("owner".to_string(), LwwRegister::new("Alice".to_string()))
        .unwrap();
    map2.insert("doc-1".to_string(), inner2).unwrap();

    // MERGE — this is the critical operation
    Mergeable::merge(&mut map1, &map2).unwrap();

    // Verify: ALL keys are present after merge.
    // The "initial" key exists on both nodes with the same string value "value";
    // this test checks key *presence* only — which replica's LwwRegister wins
    // the tiebreak is intentionally irrelevant here (both hold the same string).
    let final_inner = map1.get(&"doc-1".to_string()).unwrap().unwrap();

    assert_eq!(
        final_inner
            .get(&"initial".to_string())
            .unwrap()
            .map(|r| r.get().clone()),
        Some("value".to_string()),
        "Initial value should be preserved"
    );
    assert_eq!(
        final_inner
            .get(&"title".to_string())
            .unwrap()
            .map(|r| r.get().clone()),
        Some("Updated Title".to_string()),
        "Title update from Node 1 should be preserved"
    );
    assert_eq!(
        final_inner
            .get(&"owner".to_string())
            .unwrap()
            .map(|r| r.get().clone()),
        Some("Alice".to_string()),
        "Owner update from Node 2 should be preserved"
    );
}

#[test]
#[serial]
fn test_map_of_counters_merge() {
    env::reset_for_testing();

    // The executor ID is set *before* each block of increments so that
    // Counter::increment() records the correct per-replica key in the GCounter.
    // UnorderedMap::insert() does not consume the executor ID; it is only used
    // by Counter::increment() (and decrement()) when writing GCounter entries.

    // Node 1 (executor [100;32]): increment "counter1" three times
    env::set_executor_id([100; 32]);
    let mut map1 = UnorderedMap::<String, Counter>::new();
    let mut c1 = Counter::new();
    c1.increment().unwrap();
    c1.increment().unwrap();
    c1.increment().unwrap(); // positive[[100;32]] = 3
    map1.insert("counter1".to_string(), c1).unwrap();

    // Node 2 (executor [200;32]): also increment "counter1" three times (concurrent)
    env::set_executor_id([200; 32]);
    let mut map2 = UnorderedMap::<String, Counter>::new();
    let mut c2 = Counter::new();
    c2.increment().unwrap();
    c2.increment().unwrap();
    c2.increment().unwrap(); // positive[[200;32]] = 3
    map2.insert("counter1".to_string(), c2).unwrap();

    // MERGE — GCounter takes max per executor, so [100]=3 + [200]=3 → total = 6
    Mergeable::merge(&mut map1, &map2).unwrap();

    let final_counter = map1
        .get(&"counter1".to_string())
        .unwrap()
        .unwrap()
        .into_inner();
    assert_eq!(
        final_counter.value().unwrap(),
        6,
        "Counters should sum across executors: 3 + 3 = 6"
    );
}

#[test]
#[serial]
fn test_map_of_lww_registers_merge() {
    env::reset_for_testing();

    // Use explicit logical timestamps with distinct node IDs so the test is
    // deterministic (no wall-clock dependency) and exercises realistic
    // per-node ID tiebreaking semantics.
    let node1 = NonZeroU128::new(1).unwrap();
    let node2 = NonZeroU128::new(2).unwrap();

    // Pin the ordering assumption: NTP64(200) > NTP64(100) regardless of node ID.
    assert!(
        ts(200, node2) > ts(100, node1),
        "timestamp ordering assumption violated"
    );

    // Node 1: timestamp 100, node ID 1
    let mut map1 = UnorderedMap::<String, LwwRegister<String>>::new();
    map1.insert(
        "title".to_string(),
        LwwRegister::new_with_metadata("From Node 1".to_string(), ts(100, node1), [1u8; 32]),
    )
    .unwrap();

    // Node 2: timestamp 200, node ID 2 (explicitly later — must win)
    let mut map2 = UnorderedMap::<String, LwwRegister<String>>::new();
    map2.insert(
        "title".to_string(),
        LwwRegister::new_with_metadata("From Node 2".to_string(), ts(200, node2), [2u8; 32]),
    )
    .unwrap();

    // MERGE — LWW: latest timestamp wins
    Mergeable::merge(&mut map1, &map2).unwrap();

    let final_title = map1
        .get(&"title".to_string())
        .unwrap()
        .unwrap()
        .into_inner();
    assert_eq!(
        final_title.get(),
        "From Node 2",
        "Latest LWW register should win"
    );
}

#[test]
#[serial]
fn test_three_level_nesting_merge() {
    env::reset_for_testing();

    type InnerMap = UnorderedMap<String, LwwRegister<String>>;
    type OuterMap = UnorderedMap<String, InnerMap>;

    // Node 1: doc-1 has "initial" + "title"
    let mut map1 = OuterMap::new();
    let mut inner1 = InnerMap::new();
    inner1
        .insert("initial".to_string(), LwwRegister::new("value".to_string()))
        .unwrap();
    inner1
        .insert("title".to_string(), LwwRegister::new("Title 1".to_string()))
        .unwrap();
    map1.insert("doc-1".to_string(), inner1).unwrap();

    // Node 2: doc-1 has "initial" + "owner" (concurrent modification)
    let mut map2 = OuterMap::new();
    let mut inner2 = InnerMap::new();
    inner2
        .insert("initial".to_string(), LwwRegister::new("value".to_string()))
        .unwrap();
    inner2
        .insert("owner".to_string(), LwwRegister::new("Alice".to_string()))
        .unwrap();
    map2.insert("doc-1".to_string(), inner2).unwrap();

    // MERGE
    Mergeable::merge(&mut map1, &map2).unwrap();

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
#[serial]
fn test_map_merge_with_different_keys() {
    env::reset_for_testing();

    let mut map1 = UnorderedMap::<String, Counter>::new();
    let mut map2 = UnorderedMap::<String, Counter>::new();

    // counter_a and counter_b live under different map keys, so the merge is a
    // pure key union with no overlap. Using the same executor ID is safe here
    // because the two counters never share a key — their GCounter entries are
    // independent. If the same key appeared in both maps, the shared executor ID
    // would cause GCounter to take max(1,1)=1 instead of summing; that scenario
    // is covered by test_map_of_counters_merge.
    let mut ca = Counter::new();
    ca.increment().unwrap();
    map1.insert("counter_a".to_string(), ca).unwrap();

    // Node 2: add counter_b (2 increments)
    let mut cb = Counter::new();
    cb.increment().unwrap();
    cb.increment().unwrap();
    map2.insert("counter_b".to_string(), cb).unwrap();

    // MERGE — both keys should appear in map1
    Mergeable::merge(&mut map1, &map2).unwrap();

    assert_eq!(
        map1.get(&"counter_a".to_string())
            .unwrap()
            .unwrap()
            .into_inner()
            .value()
            .unwrap(),
        1
    );
    assert_eq!(
        map1.get(&"counter_b".to_string())
            .unwrap()
            .unwrap()
            .into_inner()
            .value()
            .unwrap(),
        2
    );
}
