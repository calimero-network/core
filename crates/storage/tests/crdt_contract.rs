//! Generic CRDT property tests.
//!
//! Every collection that implements [`Mergeable`] (or one of the shape sub-traits
//! [`CrdtMap`], [`CrdtSequence`], [`CrdtSet`]) is exercised here through the trait
//! surface only — no per-type tests. The CRDT laws checked are:
//!
//! - **Idempotency:** redelivering an update does not change state — i.e.
//!   `merge(merge(a, b), b) == merge(a, b)`. This is the law that actually catches
//!   double-counting bugs (a counter that increments per re-merge instead of
//!   converging).
//! - **Commutativity:** `merge(a, b) == merge(b, a)` (order doesn't matter).
//! - **Associativity:** `merge(merge(a, b), c) == merge(a, merge(b, c))` (grouping doesn't matter).
//!
//! Together these guarantee convergence: any set of replicas applying the same
//! set of updates in any order reaches the same final state.
//!
//! ## How additional test files would reuse this
//!
//! Each `tests/*.rs` is its own crate, so [`assert_crdt_laws`] is **not** importable
//! from a sibling integration-test file even if it were `pub`. Per-collection
//! contract tests live as additional `#[test]` functions appended to *this* file
//! so they can call the helper directly. If a future PR needs the helper from
//! another file, promote it into a `tests/common/mod.rs` first — making it `pub`
//! here would not be enough.

use calimero_storage::collections::{CrdtMap, CrdtSequence, CrdtSet, Mergeable};

// Compile-time assertions: a missing sub-trait impl shows up as a build error
// here instead of a confusing test-time panic. These functions are never
// called — the trait-bound check happens during type-checking, which is what
// catches the regression. `#[allow(dead_code)]` silences the otherwise-correct
// warning that the function body is never executed.
#[allow(dead_code)]
fn _vector_implements_crdt_sequence() {
    fn _assert<T: CrdtSequence>() {}
    _assert::<
        calimero_storage::collections::Vector<
            calimero_storage::collections::LwwRegister<String>,
            calimero_storage::store::MainStorage,
        >,
    >();
}

#[allow(dead_code)]
fn _unordered_set_implements_crdt_set() {
    fn _assert<T: CrdtSet>() {}
    _assert::<
        calimero_storage::collections::UnorderedSet<String, calimero_storage::store::MainStorage>,
    >();
}

#[allow(dead_code)]
fn _unordered_map_implements_crdt_map() {
    fn _assert<T: CrdtMap>() {}
    _assert::<
        calimero_storage::collections::UnorderedMap<
            String,
            calimero_storage::collections::Counter,
            calimero_storage::store::MainStorage,
        >,
    >();
}

/// Run the three CRDT laws against constructors that produce fresh instances.
///
/// # Determinism contract
///
/// `make_a`, `make_b`, `make_c` must each return **structurally equal** values on
/// every call. The associativity and commutativity laws call `make_a()` twice (and
/// likewise for the others) and compare the resulting merge products via `eq`. If a
/// constructor injects fresh per-call data (random actor IDs, current timestamps,
/// nondeterministic node identifiers), the comparison becomes meaningless and the
/// test will spuriously fail. Pin actor IDs and timestamps explicitly inside the
/// constructor closure.
///
/// # Parameters
///
/// - `make_a` / `make_b` / `make_c`: zero-arg constructors. Must be deterministic
///   per the contract above.
/// - `eq`: state-equality closure. Most collections can't derive `PartialEq`
///   cheaply (storage I/O), so it's supplied per-type — it might enumerate entries
///   via `.entries()`, sort and compare, etc.
fn assert_crdt_laws<T, A, B, C, E>(make_a: A, make_b: B, make_c: C, eq: E)
where
    T: Mergeable,
    A: Fn() -> T,
    B: Fn() -> T,
    C: Fn() -> T,
    E: Fn(&T, &T) -> bool,
{
    // Idempotency under redelivery: merge(merge(a, b), b) == merge(a, b).
    // This is the law that catches "double-counting" bugs — e.g. a counter that
    // re-applies an increment when the same delta arrives twice.
    //
    // Storage-backed CRDTs (Vector, UnorderedMap, ...) don't implement `Clone`
    // — cloning would mean deep-copying their backing storage — so instead of
    // `clone()`-ing the once-merged value we materialise the twice-merged value
    // from fresh constructors. The determinism contract on `make_a`/`make_b`
    // guarantees the two pipelines start from structurally equal inputs.
    {
        let mut once = make_a();
        let b = make_b();
        once.merge(&b).expect("merge a<-b must not fail");

        let mut twice = make_a();
        let b2 = make_b();
        twice
            .merge(&b2)
            .expect("merge a<-b (twice pipeline) must not fail");
        let b3 = make_b();
        twice
            .merge(&b3)
            .expect("merge (a+b)<-b (redelivery) must not fail");

        assert!(
            eq(&once, &twice),
            "idempotency violated: redelivering b after merge(a, b) changed state"
        );
    }

    // Commutativity: merge(a, b) == merge(b, a)
    {
        let mut ab = make_a();
        let b = make_b();
        ab.merge(&b).expect("merge a<-b must not fail");

        let mut ba = make_b();
        let a = make_a();
        ba.merge(&a).expect("merge b<-a must not fail");

        assert!(
            eq(&ab, &ba),
            "commutativity violated: merge(a, b) != merge(b, a)"
        );
    }

    // Associativity: merge(merge(a, b), c) == merge(a, merge(b, c))
    {
        let mut left = make_a();
        let b = make_b();
        left.merge(&b).expect("merge a<-b must not fail");
        let c = make_c();
        left.merge(&c).expect("merge (a+b)<-c must not fail");

        let mut right = make_a();
        let mut bc = make_b();
        let c2 = make_c();
        bc.merge(&c2).expect("merge b<-c must not fail");
        right.merge(&bc).expect("merge a<-(b+c) must not fail");

        assert!(eq(&left, &right), "associativity violated");
    }
}

#[test]
fn scaffold_file_compiles() {
    // Smoke test: this file builds. Real impl tests land in PR-B.
}

#[test]
fn vector_with_lww_register_satisfies_crdt_laws() {
    use calimero_storage::collections::{LwwRegister, Vector};
    use calimero_storage::env;
    use calimero_storage::logical_clock::HybridTimestamp;
    use calimero_storage::store::MainStorage;

    env::reset_for_testing();

    // Pin timestamp + node_id per builder so two `fresh(name)` calls return
    // structurally identical registers. Otherwise `LwwRegister::new` reads the
    // HLC and `make_a()`'s second invocation drifts forward, breaking the
    // determinism contract `assert_crdt_laws` requires. Using `zero()` time
    // forces the merge tie-breaker onto node_id, which is fixed per `name`.
    fn fresh(name: &str, node: [u8; 32]) -> Vector<LwwRegister<String>, MainStorage> {
        let mut v = Vector::new();
        v.push(LwwRegister::new_with_metadata(
            name.to_owned(),
            HybridTimestamp::zero(),
            node,
        ))
        .unwrap();
        v
    }

    let eq = |a: &Vector<LwwRegister<String>, MainStorage>,
              b: &Vector<LwwRegister<String>, MainStorage>|
     -> bool {
        let la = a.len().unwrap();
        let lb = b.len().unwrap();
        if la != lb {
            return false;
        }
        for i in 0..la {
            let va = a.get(i).unwrap();
            let vb = b.get(i).unwrap();
            if va.as_ref().map(|r| r.get().clone()) != vb.as_ref().map(|r| r.get().clone()) {
                return false;
            }
        }
        true
    };

    assert_crdt_laws(
        || fresh("alice", [11; 32]),
        || fresh("bob", [22; 32]),
        || fresh("carol", [33; 32]),
        eq,
    );
}

#[test]
fn unordered_set_satisfies_crdt_laws() {
    use calimero_storage::collections::UnorderedSet;
    use calimero_storage::env;
    use calimero_storage::store::MainStorage;

    env::reset_for_testing();

    fn fresh(items: &[&str]) -> UnorderedSet<String, MainStorage> {
        let mut s = UnorderedSet::new();
        for item in items {
            s.insert((*item).to_owned()).unwrap();
        }
        s
    }

    let eq = |a: &UnorderedSet<String, MainStorage>, b: &UnorderedSet<String, MainStorage>| {
        let mut a_items: Vec<_> = a.iter().unwrap().collect();
        let mut b_items: Vec<_> = b.iter().unwrap().collect();
        a_items.sort();
        b_items.sort();
        a_items == b_items
    };

    assert_crdt_laws(
        || fresh(&["alice", "bob"]),
        || fresh(&["bob", "carol"]),
        || fresh(&["dave"]),
        eq,
    );
}

#[test]
fn unordered_map_with_counter_satisfies_crdt_laws() {
    use calimero_storage::collections::{Counter, UnorderedMap};
    use calimero_storage::env;
    use calimero_storage::store::MainStorage;

    env::reset_for_testing();

    // Each replica writes both a shared key (`"shared"`) AND a private key.
    // - The shared key forces UnorderedMap::merge to recursively merge the
    //   nested Counter values, which is where the per-actor max-merge
    //   conflict resolution actually runs.
    // - The private keys ensure add-wins union behaviour is also exercised.
    // Without the shared key the test would pass trivially: disjoint-keys
    // merges never conflict.
    let make = |executor: [u8; 32],
                private_key: &'static str,
                shared_count: usize,
                private_count: usize| {
        move || {
            env::set_executor_id(executor);
            let mut m = UnorderedMap::new();

            // Shared key — every replica writes to it under its own actor.
            let mut shared = Counter::<false, MainStorage>::new();
            for _ in 0..shared_count {
                shared.increment().unwrap();
            }
            m.insert("shared".to_owned(), shared).unwrap();

            // Private key — only this replica writes to it.
            let mut private = Counter::<false, MainStorage>::new();
            for _ in 0..private_count {
                private.increment().unwrap();
            }
            m.insert(private_key.to_owned(), private).unwrap();
            m
        }
    };

    let eq = |a: &UnorderedMap<String, Counter, MainStorage>,
              b: &UnorderedMap<String, Counter, MainStorage>| {
        let mut a_entries: Vec<(String, u64)> = a
            .entries()
            .unwrap()
            .map(|(k, v)| (k, v.value().unwrap()))
            .collect();
        let mut b_entries: Vec<(String, u64)> = b
            .entries()
            .unwrap()
            .map(|(k, v)| (k, v.value().unwrap()))
            .collect();
        a_entries.sort();
        b_entries.sort();
        a_entries == b_entries
    };

    assert_crdt_laws(
        make([11; 32], "alice", 2, 1),
        make([22; 32], "bob", 3, 1),
        make([33; 32], "carol", 5, 1),
        eq,
    );
}

#[test]
fn lww_register_satisfies_crdt_laws() {
    use calimero_storage::collections::LwwRegister;
    use calimero_storage::env;
    use calimero_storage::logical_clock::HybridTimestamp;

    env::reset_for_testing();

    // Same pinned-metadata trick as the Vector test: `LwwRegister::new` reads
    // the live HLC on every call, which violates the determinism contract.
    // Using `HybridTimestamp::zero()` for everyone forces the tie-breaker onto
    // node_id — which is fixed per builder — so merges converge deterministically.
    fn fresh(name: &str, node: [u8; 32]) -> LwwRegister<String> {
        LwwRegister::new_with_metadata(name.to_owned(), HybridTimestamp::zero(), node)
    }

    let eq = |a: &LwwRegister<String>, b: &LwwRegister<String>| a.get() == b.get();

    assert_crdt_laws(
        || fresh("alice", [11; 32]),
        || fresh("bob", [22; 32]),
        || fresh("carol", [33; 32]),
        eq,
    );

    // Additional check on equal-timestamp tie-breaking: with all three
    // timestamps pinned to zero, the merge must converge on the value carried
    // by the *highest* node_id (the documented LWW tie-breaker). The
    // commutativity check inside `assert_crdt_laws` only proves
    // `merge(a, b) == merge(b, a)`, not which side wins — so a buggy impl
    // that systematically picks the *lower* node_id would still pass
    // commutativity but break the semantic contract.
    let mut r1 = fresh("alice", [11; 32]);
    let r3 = fresh("carol", [33; 32]);
    r1.merge(&r3);
    assert_eq!(
        r1.get(),
        "carol",
        "LWW tie-break: higher node_id ([33;32]) must win at equal timestamps"
    );

    let mut r3b = fresh("carol", [33; 32]);
    let r1b = fresh("alice", [11; 32]);
    r3b.merge(&r1b);
    assert_eq!(
        r3b.get(),
        "carol",
        "LWW tie-break must be order-independent: higher node_id wins from either direction"
    );
}

#[test]
fn counter_satisfies_crdt_laws() {
    use calimero_storage::collections::Counter;
    use calimero_storage::env;
    use calimero_storage::store::MainStorage;

    env::reset_for_testing();

    // Each replica increments under its own private executor AND under a
    // single shared executor with replica-specific counts. The shared
    // executor is what exercises the *per-actor max-merge* conflict logic
    // — without it every test slot would be disjoint and commutativity
    // would hold trivially. With it, `merge(a, b)` must take the max of
    // the shared-executor count from both sides regardless of merge order.
    const SHARED_EXECUTOR: [u8; 32] = [99; 32];

    let make = |private_executor: [u8; 32], private_count: usize, shared_count: usize| {
        move || {
            let mut c = Counter::<false, MainStorage>::new();

            // Increments under the shared executor (the conflict-resolution slot).
            env::set_executor_id(SHARED_EXECUTOR);
            for _ in 0..shared_count {
                c.increment().unwrap();
            }

            // Increments under this replica's private executor.
            env::set_executor_id(private_executor);
            for _ in 0..private_count {
                c.increment().unwrap();
            }
            c
        }
    };

    let eq = |a: &Counter<false, MainStorage>, b: &Counter<false, MainStorage>| {
        a.value().unwrap() == b.value().unwrap()
    };

    // Shared-slot counts: 2, 7, 4 — max under merge is 7 (regardless of order).
    // Private counts: 2, 3, 5 — non-overlapping executors, always summed.
    assert_crdt_laws(
        make([11; 32], 2, 2),
        make([22; 32], 3, 7),
        make([33; 32], 5, 4),
        eq,
    );
}

// RGA generates fresh per-character ids on each `insert_str` call, so two
// `make_a()` invocations produce disjoint character sets. Merging those
// disjoint sets doubles the content, which violates structural equality
// across runs — the helper's determinism contract cannot be satisfied
// without bypassing RGA's own non-determinism. Rather than fight the
// model, we ignore this test and document the reason inline so a future
// refactor (e.g. deterministic CharId seeding) can revive it. Mergeable
// for RGA is still covered by the existing tests in `crdt_impls.rs`.
#[test]
#[ignore = "RGA inserts allocate fresh per-character ids; two `make_*()` calls produce disjoint id sets, breaking the determinism contract `assert_crdt_laws` requires. Mergeable laws for RGA are exercised by `test_rga_merge_*` in src/collections/crdt_impls.rs."]
fn rga_satisfies_crdt_laws() {
    use calimero_storage::collections::ReplicatedGrowableArray;
    use calimero_storage::env;

    env::reset_for_testing();

    fn fresh(s: &str) -> ReplicatedGrowableArray {
        let mut r = ReplicatedGrowableArray::new();
        r.insert_str(0, s).unwrap();
        r
    }

    let eq = |a: &ReplicatedGrowableArray, b: &ReplicatedGrowableArray| {
        a.len().unwrap() == b.len().unwrap()
    };

    assert_crdt_laws(|| fresh("aa"), || fresh("bb"), || fresh("cc"), eq);
}
