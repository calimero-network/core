#![allow(unused_crate_dependencies, reason = "False positives")]
#![allow(
    clippy::tests_outside_test_module,
    reason = "Allowable in integration tests"
)]

// Pins the code the SDK macros generate for valid input; the trybuild suite in
// `macros.rs` covers the error side. Expanded for wasm32, where the exported
// entry points and merge hooks exist. Needs `cargo-expand` on PATH and the
// wasm32 target. After an intentional macro or toolchain change, re-bless with:
//   MACROTEST=overwrite cargo test -p calimero-sdk --test expand
// then review the golden diff.
#[test]
fn expansion() {
    macrotest::expand_args("tests/expand/*.rs", ["--target", "wasm32-unknown-unknown"]);
}
