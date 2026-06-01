//! Convergence coverage for the broadest CRDT app: counters, registers, sets,
//! vectors, sorted maps. Runs in its own integration-test binary for isolation.

use nested_crdt_test::NestedCrdtTest;

use calimero_storage::testing::converge_app;

// Direct `Counter` map values: concurrent increments must SUM (one per replica).
#[test]
fn counters_sum_across_replicas() {
    converge_app(NestedCrdtTest::init)
        .replicas(3)
        .ops(|s| {
            let _ = s.increment_counter("hits".into());
        })
        .invariant("hits == 3 (one increment per replica)", |s| {
            s.get_counter("hits".into()).unwrap_or(0) == 3
        })
        .assert_all_replicas_equal();
}

// `UnorderedSet` values: concurrent adds of distinct tags must UNION.
#[test]
fn set_union_across_replicas() {
    converge_app(NestedCrdtTest::init)
        .replicas(3)
        .ops(|s| {
            let _ = s.add_tag("post".into(), "rust".into());
        })
        .ops(|s| {
            let _ = s.add_tag("post".into(), "crdt".into());
        })
        .invariant("post has 2 tags (union of rust + crdt)", |s| {
            s.get_tag_count("post".into()).unwrap_or(0) == 2
        })
        .assert_all_replicas_equal();
}

// Mixed shapes (register LWW, vector push, sorted map) must converge.
#[test]
fn mixed_crdt_shapes_converge() {
    converge_app(NestedCrdtTest::init)
        .replicas(4)
        .seed(7)
        .ops(|s| {
            let _ = s.set_register("title".into(), "hello".into());
        })
        .ops(|s| {
            let _ = s.set_sorted_score("alice".into(), 10);
        })
        .ops(|s| {
            let _ = s.push_metric(5);
        })
        .assert_all_replicas_equal();
}
