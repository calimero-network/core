#[test]
fn all() {
    let t = trybuild::TestCases::new();

    t.pass("tests/macros/valid_receivers.rs");
    t.compile_fail("tests/macros/invalid_receivers.rs");
    t.compile_fail("tests/macros/generics.rs");
    t.pass("tests/macros/valid_args.rs");
    t.compile_fail("tests/macros/invalid_args.rs");
    t.compile_fail("tests/macros/invalid_methods.rs");
}
