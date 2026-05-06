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
    t.compile_fail("tests/compile_fail/*.rs");
}
