//! End-to-end coverage for the `#[derive(Mergeable)]` proc-macro.
//!
//! Lives under `tests/` because the derive lands in `calimero-sdk-macros`
//! (re-exported via `calimero-sdk::app::Mergeable`) but emits paths into
//! `calimero-storage`. The integration test binary links both crates and
//! verifies the generated impl actually merges field-by-field.

use calimero_sdk::app::Mergeable;
use calimero_storage::collections::{Counter, LwwRegister, Mergeable as _};

#[derive(Mergeable)]
struct UserStats {
    visits: Counter,
    name: LwwRegister<String>,
}

#[test]
fn derive_emits_field_by_field_merge() {
    let mut a = UserStats {
        visits: Counter::new(),
        name: LwwRegister::new("alice".to_owned()),
    };
    a.visits.increment().unwrap();

    let mut b = UserStats {
        visits: Counter::new(),
        name: LwwRegister::new("bob".to_owned()),
    };
    b.visits.increment().unwrap();
    b.visits.increment().unwrap();

    a.merge(&b).unwrap();

    // Counter takes max-per-executor, so after merge the visible total covers
    // each replica's local count.
    let total = a.visits.value().unwrap();
    assert!(
        total >= 2,
        "merged counter should reflect at least b's contribution, got {total}"
    );
}

#[derive(Mergeable)]
struct TupleWrapper(LwwRegister<u64>, Counter);

#[test]
fn derive_supports_tuple_structs() {
    let mut a = TupleWrapper(LwwRegister::new(1), Counter::new());
    let mut b = TupleWrapper(LwwRegister::new(2), Counter::new());
    b.1.increment().unwrap();
    b.1.increment().unwrap();

    a.merge(&b).unwrap();

    assert!(a.1.value().unwrap() >= 2);
}

#[derive(Mergeable)]
struct EmptyMarker;

#[test]
fn derive_supports_unit_structs() {
    let mut a = EmptyMarker;
    let b = EmptyMarker;
    a.merge(&b).unwrap();
}

// Regression coverage for the silent-skip bug in `#[app::state]`'s generated
// merge lives in `crates/sdk/macros/src/state.rs::tests` (the generator is a
// private function and easier to inspect directly via TokenStream).

#[derive(Mergeable)]
struct BoxedField {
    boxed_counter: Box<Counter>,
}

#[test]
fn box_delegates_merge_to_inner() {
    // Sanity check that `Mergeable for Box<T>` actually composes with the
    // derive — the lint's `pass_through` list claims Box is OK at the type
    // level, this confirms the impl exists and merges field-by-field.
    let mut a = BoxedField {
        boxed_counter: Box::new(Counter::new()),
    };
    let mut b = BoxedField {
        boxed_counter: Box::new(Counter::new()),
    };
    b.boxed_counter.increment().unwrap();

    a.merge(&b).unwrap();

    assert!(a.boxed_counter.value().unwrap() >= 1);
}
