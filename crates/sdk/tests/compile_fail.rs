//! Declarations the `app` macros refuse at compile time.

#![cfg(not(target_arch = "wasm32"))]

#[test]
fn refused_declarations() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
