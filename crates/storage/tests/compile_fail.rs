//! Negative tests for the `#[app::state]` and `#[derive(Mergeable)]` lints.
//!
//! Each `tests/compile_fail/*.rs` is a tiny crate that *should fail* to
//! compile because it triggers the forbidden-type lint. The matching
//! `*.stderr` file captures the expected error output. To regenerate after
//! intentional message changes, run:
//!
//!     TRYBUILD=overwrite cargo test --test compile_fail
//!
//! Coverage is intentionally narrow: one rejection path per file. If you add
//! a new lint case, add a focused test here so the failure mode stays
//! discoverable in review.

#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();

    // Feature-stable cases: their `.stderr` is identical regardless of which
    // crate features are active, so they run under every feature set.
    t.compile_fail("tests/compile_fail/derive_mergeable_enum.rs");
    t.compile_fail("tests/compile_fail/derive_mergeable_hashmap.rs");
    t.compile_fail("tests/compile_fail/state_bare_primitive.rs");
    t.compile_fail("tests/compile_fail/state_bare_string.rs");
    t.compile_fail("tests/compile_fail/state_hashmap_field.rs");
    t.compile_fail("tests/compile_fail/state_hashmap_in_lww.rs");

    // Feature-SENSITIVE case. rustc's "the following other types implement
    // trait `RekeyTarget`" help block lists implementing types alphabetically
    // and truncates after 8 (trybuild collapses the rest to `and $N others`).
    // The `testing` feature pulls `tests::common::EmptyData` (and its
    // `RekeyTarget` impl) into scope; it sorts into that first-8 window and
    // displaces another type, so the captured `.stderr` differs between the
    // default and `testing` feature sets — and trybuild has no partial-line
    // wildcard to paper over the difference.
    //
    // CI runs `cargo test` over the whole workspace, where feature unification
    // (`calimero-dag`/`calimero-node` enable `calimero-storage/testing`) turns
    // `testing` ON for this crate's own test binary too. So the captured
    // `.stderr` is blessed for the `testing`-ON output, and the case is gated to
    // run only when that feature is active — matching CI exactly while keeping
    // the default-feature run (`-p calimero-storage` in isolation) mismatch-free.
    #[cfg(feature = "testing")]
    t.compile_fail("tests/compile_fail/mergeable_without_rekeytarget.rs");
}
