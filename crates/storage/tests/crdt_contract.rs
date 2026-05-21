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
/// - `eq`: state-equality closure. Most collections can't derive `PartialEq`
///   cheaply (storage I/O), so it's supplied per-type — it might enumerate entries
///   via `.entries()`, sort and compare, etc.
pub fn assert_crdt_laws<T, F, E>(make_a: F, make_b: F, make_c: F, eq: E)
where
    T: Mergeable + Clone,
    F: Fn() -> T,
    E: Fn(&T, &T) -> bool,
{
    // Idempotency under redelivery: merge(merge(a, b), b) == merge(a, b).
    // This is the law that catches "double-counting" bugs — e.g. a counter that
    // re-applies an increment when the same delta arrives twice.
    {
        let mut once = make_a();
        let b = make_b();
        once.merge(&b).expect("merge a<-b must not fail");

        let mut twice = once.clone();
        twice
            .merge(&b)
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
