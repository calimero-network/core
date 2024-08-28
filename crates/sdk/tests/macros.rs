#![allow(unused_crate_dependencies)]

#[ignore]
#[test]
fn all() {
    let t = trybuild::TestCases::new();

    // todo! break these up into pass/fail dirs for organization

    t.pass("tests/macros/valid_receivers.rs");
    t.compile_fail("tests/macros/invalid_receivers.rs");
    t.compile_fail("tests/macros/invalid_generics.rs");
    t.pass("tests/macros/valid_generics.rs");
    t.pass("tests/macros/valid_args.rs");
    t.compile_fail("tests/macros/invalid_args.rs");
    t.compile_fail("tests/macros/invalid_methods.rs");
}
