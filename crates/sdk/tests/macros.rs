#![allow(unused_crate_dependencies, reason = "False positives")]
#![allow(
    clippy::tests_outside_test_module,
    reason = "Allowable in integration tests"
)]

#[ignore]
#[test]
fn all() {
    let t = trybuild::TestCases::new();

    // todo! break these up into pass/fail dirs for organization

    // Valid receiver tests
    t.pass("tests/macros/valid_receivers.rs");
    t.compile_fail("tests/macros/invalid_receivers.rs");

    // Generic tests
    t.compile_fail("tests/macros/invalid_generics.rs");
    t.pass("tests/macros/valid_generics.rs");

    // Argument tests
    t.pass("tests/macros/valid_args.rs");
    t.compile_fail("tests/macros/invalid_args.rs");

    // Method tests
    t.compile_fail("tests/macros/invalid_methods.rs");

    // === Edge case tests ===

    // Nested types tests
    t.pass("tests/macros/nested_types.rs");

    // Complex generics tests
    t.pass("tests/macros/complex_generics_valid.rs");
    t.compile_fail("tests/macros/invalid_nested_generics.rs");

    // Attribute interaction tests
    t.pass("tests/macros/attribute_interactions.rs");

    // Event tests
    t.pass("tests/macros/valid_events.rs");

    // Error message quality tests
    t.compile_fail("tests/macros/error_init_not_named.rs");
    t.compile_fail("tests/macros/error_init_without_attr.rs");
    t.compile_fail("tests/macros/error_private_event.rs");
    t.compile_fail("tests/macros/error_init_with_self.rs");
    t.compile_fail("tests/macros/error_trait_impl.rs");
    t.compile_fail("tests/macros/error_impl_trait_arg.rs");
    t.compile_fail("tests/macros/error_const_generic.rs");
    t.compile_fail("tests/macros/error_explicit_abi.rs");
}
