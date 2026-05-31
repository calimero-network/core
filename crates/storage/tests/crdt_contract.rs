//! Generic CRDT property tests.
//!
//! Every collection that implements [`Mergeable`] is exercised here through that
//! trait surface only — no per-type tests. The CRDT laws checked are:
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
//! Each `tests/*.rs` is its own crate, so [`assert_mergeable_laws`] is **not**
//! importable from a sibling integration-test file even if it were `pub`.
//! Per-collection contract tests live as additional `#[test]` functions appended
//! to *this* file so they can call the helper directly. If a future PR needs the
//! helper from another file, promote it into a `tests/common/mod.rs` first —
//! making it `pub` here would not be enough.

use calimero_storage::collections::Mergeable;

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
/// - `eq`: state-equality closure. Most storage-backed collections can't derive
///   `PartialEq` cheaply (storage I/O), so it's supplied per-type — it might
///   enumerate entries via `.entries()`, sort and compare, etc.
fn assert_mergeable_laws<T, A, B, C, E>(make_a: A, make_b: B, make_c: C, eq: E)
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
fn vector_with_lww_register_satisfies_crdt_laws() {
    use calimero_storage::collections::{LwwRegister, Vector};
    use calimero_storage::logical_clock::HybridTimestamp;
    use calimero_storage::store::MainStorage;

    // Pin timestamp + node_id per builder so two `fresh(name)` calls return
    // structurally identical registers. Otherwise `LwwRegister::new` reads the
    // HLC and `make_a()`'s second invocation drifts forward, breaking the
    // determinism contract `assert_mergeable_laws` requires. Using `zero()` time
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

    assert_mergeable_laws(
        || fresh("alice", [11; 32]),
        || fresh("bob", [22; 32]),
        || fresh("carol", [33; 32]),
        eq,
    );
}

#[test]
fn unordered_set_satisfies_crdt_laws() {
    use calimero_storage::collections::UnorderedSet;
    use calimero_storage::store::MainStorage;

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

    assert_mergeable_laws(
        || fresh(&["alice", "bob"]),
        || fresh(&["bob", "carol"]),
        || fresh(&["dave"]),
        eq,
    );
}

// Disjoint-keys merge for UnorderedMap<String, Counter> with no shared-key
// conflict slot. Establishes add-wins union over keys without touching the
// per-actor max-merge conflict path. The shared-key conflict variant lives
// below as `unordered_map_with_counter_shared_key_conflict`, which exercises
// the per-actor max-merge slot via the `env::with_executor_id` scoped guard.
#[test]
fn unordered_map_with_counter_satisfies_crdt_laws() {
    use calimero_storage::collections::{Counter, UnorderedMap};
    use calimero_storage::store::MainStorage;

    let make = |private_key: &'static str, private_count: usize| {
        move || {
            let mut m = UnorderedMap::new();

            // Each replica writes to its own private key only — disjoint keys
            // exercise add-wins union behaviour deterministically without
            // needing per-actor executor mutation.
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

    assert_mergeable_laws(make("alice", 1), make("bob", 1), make("carol", 1), eq);
}

// Shared-key + per-replica executor conflict variant — the path that
// actually exercises UnorderedMap's recursive merge into a nested
// Counter on the same key from different replicas. Each replica writes
// the shared key under its own `executor_id` via the scoped
// [`env::with_executor_id`] guard, which restores prior identity even
// on panic so a failure here doesn't pollute siblings in the same
// process.
#[test]
fn unordered_map_with_counter_shared_key_conflict() {
    use calimero_storage::collections::{Counter, UnorderedMap};
    use calimero_storage::env;
    use calimero_storage::store::MainStorage;

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
            env::with_executor_id(executor, || {
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
            })
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

    assert_mergeable_laws(
        make([11; 32], "alice", 2, 1),
        make([22; 32], "bob", 3, 1),
        make([33; 32], "carol", 5, 1),
        eq,
    );
}

#[test]
fn lww_register_satisfies_crdt_laws() {
    use calimero_storage::collections::LwwRegister;
    use calimero_storage::logical_clock::HybridTimestamp;

    // Same pinned-metadata trick as the Vector test: `LwwRegister::new` reads
    // the live HLC on every call, which violates the determinism contract.
    // Using `HybridTimestamp::zero()` for everyone forces the tie-breaker onto
    // node_id — which is fixed per builder — so merges converge deterministically.
    fn fresh(name: &str, node: [u8; 32]) -> LwwRegister<String> {
        LwwRegister::new_with_metadata(name.to_owned(), HybridTimestamp::zero(), node)
    }

    let eq = |a: &LwwRegister<String>, b: &LwwRegister<String>| a.get() == b.get();

    assert_mergeable_laws(
        || fresh("alice", [11; 32]),
        || fresh("bob", [22; 32]),
        || fresh("carol", [33; 32]),
        eq,
    );

    // Additional check on equal-timestamp tie-breaking: with all three
    // timestamps pinned to zero, the merge must converge on the value carried
    // by the *highest* node_id (the documented LWW tie-breaker). The
    // commutativity check inside `assert_mergeable_laws` only proves
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

// Counter shared-executor max-merge conflict: each replica increments
// under its own private executor AND under a shared executor (the
// per-actor conflict slot). With shared-slot counts {2, 7, 4} the
// max-under-merge is 7 regardless of merge order; private slots simply
// sum (disjoint executors).
//
// Driven by the `env::with_executor_id` scoped guard so the
// per-replica increments run under the right `executor_id`, with prior
// identity restored even on panic so a failure here doesn't pollute
// siblings in the same process.
#[test]
fn shared_executor_counter_merge() {
    use calimero_storage::collections::Counter;
    use calimero_storage::env;
    use calimero_storage::store::MainStorage;

    const SHARED_EXECUTOR: [u8; 32] = [99; 32];

    let make = |private_executor: [u8; 32], private_count: usize, shared_count: usize| {
        move || {
            let mut c = Counter::<false, MainStorage>::new();

            // Increments under the shared executor (the conflict-resolution slot).
            env::with_executor_id(SHARED_EXECUTOR, || {
                for _ in 0..shared_count {
                    c.increment().unwrap();
                }
            });

            // Increments under this replica's private executor.
            env::with_executor_id(private_executor, || {
                for _ in 0..private_count {
                    c.increment().unwrap();
                }
            });
            c
        }
    };

    let eq = |a: &Counter<false, MainStorage>, b: &Counter<false, MainStorage>| {
        a.value().unwrap() == b.value().unwrap()
    };

    // Shared-slot counts: 2, 7, 4 — max under merge is 7 (regardless of order).
    // Private counts: 2, 3, 5 — non-overlapping executors, always summed.
    assert_mergeable_laws(
        make([11; 32], 2, 2),
        make([22; 32], 3, 7),
        make([33; 32], 5, 4),
        eq,
    );
}

// RGA's default `insert_str` allocates fresh per-character ids from the
// live HLC, so two `make_a()` invocations produce disjoint character
// sets — that violates the structural-equality requirement of
// `assert_mergeable_laws`, even though the underlying merge IS
// idempotent/commutative/associative. We side-step the determinism
// problem by going through `insert_str_at_timestamp`, which pins the
// HLC component of every `CharId` to a caller-supplied value. With a
// stable per-replica timestamp seed, repeated `make_*()` calls produce
// byte-for-byte identical `CharId` sets, the merge laws line up, and
// we get a real contract test (instead of leaning on the per-merge
// tests in `crdt_impls.rs`).
//
// Wire-level dedup wouldn't have helped here: per
// `crates/node/src/delta_store.rs` "each delta is applied at most once
// (the DAG dedups by content-addressed `delta.id`)", so the
// in-process redelivery scenario this test exercises doesn't
// correspond to anything RGA's merge sees in production. The
// determinism gap was purely test-framing.
#[test]
fn rga_satisfies_crdt_laws() {
    use calimero_storage::collections::ReplicatedGrowableArray;
    use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};

    // Three pinned timestamps with distinct, deterministic IDs so the
    // resulting CharId sets are disjoint per builder but identical
    // between repeat calls of the same builder.
    fn pinned(seed: u128, time: u64) -> HybridTimestamp {
        let id = ID::from(std::num::NonZeroU128::new(seed).expect("seed must be non-zero"));
        HybridTimestamp::new(Timestamp::new(NTP64(time), id))
    }

    fn fresh(ts: HybridTimestamp, s: &str) -> ReplicatedGrowableArray {
        let mut r = ReplicatedGrowableArray::new();
        r.insert_str_at_timestamp(0, ts, s).unwrap();
        r
    }

    let eq = |a: &ReplicatedGrowableArray, b: &ReplicatedGrowableArray| {
        a.get_text().unwrap() == b.get_text().unwrap()
    };

    assert_mergeable_laws(
        || fresh(pinned(11, 1_000), "aa"),
        || fresh(pinned(22, 2_000), "bb"),
        || fresh(pinned(33, 3_000), "cc"),
        eq,
    );
}

// `SortedMap` stores and merges exactly like `UnorderedMap` (same inner
// collection, add-wins keys, recursive value merge); only its *iteration* is
// key-ordered. These tests pin that the merge laws hold through the
// `SortedMap` surface, and — separately — that the ordering invariant survives
// a merge regardless of merge direction.
#[test]
fn sorted_map_with_lww_register_satisfies_crdt_laws() {
    use calimero_storage::collections::{LwwRegister, SortedMap};
    use calimero_storage::logical_clock::HybridTimestamp;
    use calimero_storage::store::MainStorage;

    // Disjoint keys per replica: add-wins union, deterministic per builder.
    // Pin the register's HLC/node so repeat builder calls are byte-identical
    // (the `assert_mergeable_laws` determinism contract).
    fn fresh(key: &str, node: [u8; 32]) -> SortedMap<String, LwwRegister<String>, MainStorage> {
        let mut m = SortedMap::new();
        m.insert(
            key.to_owned(),
            LwwRegister::new_with_metadata(key.to_uppercase(), HybridTimestamp::zero(), node),
        )
        .unwrap();
        m
    }

    let eq = |a: &SortedMap<String, LwwRegister<String>, MainStorage>,
              b: &SortedMap<String, LwwRegister<String>, MainStorage>| {
        // `entries()` is already key-sorted, so a direct positional compare is
        // a sufficient equality check.
        let a_entries: Vec<(String, String)> = a
            .entries()
            .unwrap()
            .map(|(k, v)| (k, v.get().clone()))
            .collect();
        let b_entries: Vec<(String, String)> = b
            .entries()
            .unwrap()
            .map(|(k, v)| (k, v.get().clone()))
            .collect();
        a_entries == b_entries
    };

    assert_mergeable_laws(
        || fresh("alice", [11; 32]),
        || fresh("bob", [22; 32]),
        || fresh("carol", [33; 32]),
        eq,
    );
}

// Shared-key + per-replica executor conflict — proves `SortedMap` inherits
// `UnorderedMap`'s recursive merge into a nested `Counter` on the same key.
#[test]
fn sorted_map_with_counter_shared_key_conflict() {
    use calimero_storage::collections::{Counter, SortedMap};
    use calimero_storage::env;
    use calimero_storage::store::MainStorage;

    let make = |executor: [u8; 32],
                private_key: &'static str,
                shared_count: usize,
                private_count: usize| {
        move || {
            env::with_executor_id(executor, || {
                let mut m = SortedMap::new();

                let mut shared = Counter::<false, MainStorage>::new();
                for _ in 0..shared_count {
                    shared.increment().unwrap();
                }
                m.insert("shared".to_owned(), shared).unwrap();

                let mut private = Counter::<false, MainStorage>::new();
                for _ in 0..private_count {
                    private.increment().unwrap();
                }
                m.insert(private_key.to_owned(), private).unwrap();
                m
            })
        }
    };

    let eq = |a: &SortedMap<String, Counter, MainStorage>,
              b: &SortedMap<String, Counter, MainStorage>| {
        let a_entries: Vec<(String, u64)> = a
            .entries()
            .unwrap()
            .map(|(k, v)| (k, v.value().unwrap()))
            .collect();
        let b_entries: Vec<(String, u64)> = b
            .entries()
            .unwrap()
            .map(|(k, v)| (k, v.value().unwrap()))
            .collect();
        a_entries == b_entries
    };

    assert_mergeable_laws(
        make([11; 32], "alice", 2, 1),
        make([22; 32], "bob", 3, 1),
        make([33; 32], "carol", 5, 1),
        eq,
    );
}

// Convergence is necessary but not sufficient for a *sorted* map: the merged
// state must also iterate in key order, in both merge directions. Insert keys
// out of order on each replica, merge, and assert the result is sorted and
// direction-independent.
#[test]
fn sorted_map_iteration_is_sorted_after_merge() {
    use calimero_storage::collections::{LwwRegister, SortedMap};
    use calimero_storage::logical_clock::HybridTimestamp;
    use calimero_storage::store::MainStorage;

    fn replica(
        keys: &[&str],
        node: [u8; 32],
    ) -> SortedMap<String, LwwRegister<String>, MainStorage> {
        let mut m = SortedMap::new();
        for k in keys {
            m.insert(
                (*k).to_owned(),
                LwwRegister::new_with_metadata((*k).to_owned(), HybridTimestamp::zero(), node),
            )
            .unwrap();
        }
        m
    }

    let keys_of = |m: &SortedMap<String, LwwRegister<String>, MainStorage>| -> Vec<String> {
        m.keys().unwrap().collect()
    };

    // Deliberately scrambled insertion order, partially overlapping key sets.
    let mut ab = replica(&["m", "a", "z"], [1; 32]);
    let b = replica(&["b", "a", "q"], [2; 32]);
    ab.merge(&b).unwrap();

    let mut ba = replica(&["b", "a", "q"], [2; 32]);
    let a = replica(&["m", "a", "z"], [1; 32]);
    ba.merge(&a).unwrap();

    let expected = vec![
        "a".to_owned(),
        "b".to_owned(),
        "m".to_owned(),
        "q".to_owned(),
        "z".to_owned(),
    ];
    assert_eq!(keys_of(&ab), expected, "merge(a, b) must be sorted");
    assert_eq!(
        keys_of(&ba),
        expected,
        "merge(b, a) must be sorted and identical to merge(a, b)"
    );
}
