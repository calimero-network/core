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

    // Feature-SENSITIVE: rustc's "other types implement `RekeyTarget`" help block
    // lists implementors alphabetically (truncated at 8), and the `testing` feature
    // changes that set, so the `.stderr` differs between feature sets. CI builds the
    // workspace with `testing` on (feature unification), so the snapshot is blessed
    // for that and gated to it — keeping the default `-p calimero-storage` run clean.
    #[cfg(feature = "testing")]
    t.compile_fail("tests/compile_fail/mergeable_without_rekeytarget.rs");
}
