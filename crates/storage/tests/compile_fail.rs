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
    // lists implementors alphabetically and TRUNCATES AT 8, so the `.stderr` depends
    // on exactly which implementors are compiled in. Two features move that set:
    // `testing`, and `fugue-simple` (which adds `FugueTextSimple`, displacing
    // `PermissionedStorage` past the cutoff). CI builds the workspace with BOTH on —
    // `testing` explicitly, `fugue-simple` by feature unification via
    // `tools/storage-cost` — so the snapshot is blessed for that combination and
    // gated to it. Gating on `testing` alone made this pass locally and fail only in
    // CI, which is how it was missed.
    #[cfg(all(feature = "testing", feature = "fugue-simple"))]
    t.compile_fail("tests/compile_fail/mergeable_without_rekeytarget.rs");
}
