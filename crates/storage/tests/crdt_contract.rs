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
//! ## How PR-B and later test files use this
//!
//! Each `tests/*.rs` is its own crate, so [`assert_crdt_laws`] is **not** importable
//! from a sibling integration-test file. PR-B's per-collection contract tests live
//! as additional `#[test]` functions appended to *this* file so they can call the
//! helper directly. If a future PR needs the helper from another file, promote it
//! into a `tests/common/mod.rs` first.

use calimero_storage::collections::{CrdtMap, CrdtSequence, CrdtSet, Mergeable};

// Compile-time assertions: a missing sub-trait impl shows up as a build error here
// instead of a confusing test-time panic.
fn _vector_implements_crdt_sequence() {
    fn _assert<T: CrdtSequence>() {}
    _assert::<
        calimero_storage::collections::Vector<
            calimero_storage::collections::LwwRegister<String>,
            calimero_storage::store::MainStorage,
        >,
    >();
}

fn _unordered_set_implements_crdt_set() {
    fn _assert<T: CrdtSet>() {}
    _assert::<
        calimero_storage::collections::UnorderedSet<String, calimero_storage::store::MainStorage>,
    >();
}

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
pub fn assert_crdt_laws<T, A, B, C, E>(make_a: A, make_b: B, make_c: C, eq: E)
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

    let make = |executor: [u8; 32], key: &'static str, count: usize| {
        move || {
            env::set_executor_id(executor);
            let mut m = UnorderedMap::new();
            let mut c = Counter::<false, MainStorage>::new();
            for _ in 0..count {
                c.increment().unwrap();
            }
            m.insert(key.to_owned(), c).unwrap();
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
        make([11; 32], "alice", 2),
        make([22; 32], "bob", 3),
        make([33; 32], "carol", 5),
        eq,
    );
}
