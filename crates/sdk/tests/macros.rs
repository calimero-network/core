#![allow(unused_crate_dependencies, reason = "False positives")]
#![allow(
    clippy::tests_outside_test_module,
    reason = "Allowable in integration tests"
)]

// Verifies that the SDK proc-macros reject misuse with clear `(calimero)>`
// compile errors and accept valid usage. Runs under plain `cargo test`, so it
// gates CI. Golden `.stderr` files are toolchain-sensitive — after an
// intentional `rust-toolchain.toml` bump, re-bless with:
//   TRYBUILD=overwrite cargo test -p calimero-sdk --test macros
// then eyeball the diff to confirm only rustc noise (cascade errors, wording)
// changed and every `(calimero)>` message is preserved.
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
    t.compile_fail("tests/macros/error_event_on_struct.rs");
    t.compile_fail("tests/macros/error_private_incompatible.rs");

    // === SDK misuse diagnostics ===

    // `#[app::state]` must be a struct of CRDT fields.
    t.compile_fail("tests/macros/error_state_enum.rs");
    t.compile_fail("tests/macros/error_interior_mutability.rs");
    // A collection value that isn't a CRDT points at `Mergeable` / `LwwRegister`.
    t.compile_fail("tests/macros/error_non_mergeable_field.rs");
    // Only `#[app::event]` enums can be emitted.
    t.compile_fail("tests/macros/error_emit_non_event.rs");
    // Initializer + method-name rules.
    t.compile_fail("tests/macros/error_duplicate_init.rs");
    t.compile_fail("tests/macros/error_reserved_method_name.rs");
    // Discarding a read-only `get()` result is a no-op read.
    t.compile_fail("tests/macros/error_value_ref_must_use.rs");
    // `#[app::view]` is read-only — no `&mut self`.
    t.compile_fail("tests/macros/error_view_mutates.rs");
    // `emits = T` must name an `#[app::event]` type.
    t.compile_fail("tests/macros/error_emits_non_event.rs");
    // `PermissionedStorage<T, A>` policy must implement `Authorizer`.
    t.compile_fail("tests/macros/error_bad_authorizer.rs");
}
