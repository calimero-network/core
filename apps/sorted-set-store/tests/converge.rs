//! Convergence coverage for `SortedSet`. Besides the union semantics, this also
//! exercises the harness against a collection that uses the node-local ordered
//! secondary index — confirming convergence (a hash-based property over
//! `MainStorage`) holds even though that index isn't per-replica isolated.

use sorted_set_store::SortedSetStore;

use calimero_storage::testing::converge_app;

#[test]
fn distinct_elements_union() {
    converge_app(SortedSetStore::init)
        .replicas(3)
        .ops(|s| {
            let _ = s.add("alice".into());
        })
        .ops(|s| {
            let _ = s.add("bob".into());
        })
        .ops(|s| {
            let _ = s.add("carol".into());
        })
        .invariant("all three elements present (set union)", |s| {
            s.len().unwrap_or(0) == 3
        })
        .invariant("contains bob", |s| s.contains("bob").unwrap_or(false))
        .assert_all_replicas_equal();
}
