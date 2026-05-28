//! Migration function macro implementation.
//!
//! This module provides the `#[app::migrate]` procedural macro that transforms
//! a migration function into a WASM-compatible export for state migrations.
//!
//! # Migration Flow
//!
//! During a state migration:
//! 1. The node runtime loads the NEW application's WASM module
//! 2. The runtime calls the migration function exported by this macro
//! 3. The migration function reads the old state via `calimero_sdk::read_raw()`
//! 4. It transforms the data to the new schema and returns the new state
//! 5. The runtime writes the returned bytes back to the root state slot

use proc_macro2::TokenStream;
use quote::quote;
use syn::{ItemFn, ReturnType};

/// Generates the migration function implementation.
///
/// This function transforms a user-defined migration function into:
/// - A WASM export (for `wasm32` target) that:
///   - Sets up the panic hook for better error messages
///   - Registers the event emitter for the new state type (so `app::emit!` works)
///   - Executes the user's migration logic
///   - Serializes the result with borsh
///   - Returns the bytes via `value_return`
/// - The original function (for non-WASM targets) for testing
///
/// # Arguments
///
/// * `_attr` - Attribute arguments (currently unused)
/// * `item` - The function item to transform
///
/// # Returns
///
/// A `TokenStream` containing the generated code for both WASM and non-WASM targets.
pub fn migrate_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = match syn::parse2::<ItemFn>(item) {
        Ok(i) => i,
        Err(e) => return e.to_compile_error(),
    };

    let fn_name = &input.sig.ident;
    let block = &input.block;
    let vis = &input.vis;
    let attrs = &input.attrs;
    let sig = &input.sig;

    // Extract return type for the inner function signature
    let return_type = &sig.output;

    // Extract the state type from the return type for event registration.
    // The return type of a migration function is the new state struct (e.g., KvStoreV2),
    // which implements AppState and defines the Event type needed by app::emit!.
    let event_registration = match return_type {
        ReturnType::Type(_, ty) => quote! {
            ::calimero_sdk::event::register::<#ty>();
        },
        ReturnType::Default => quote! {},
    };

    quote! {
        /// WASM export for migration function.
        ///
        /// This function is called by the node runtime during application upgrades.
        /// It reads the old state, executes the migration logic, and returns the new state bytes.
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn #fn_name() {
            ::calimero_sdk::env::setup_panic_hook();

            // Register the event emitter for the new state type so app::emit! works
            // during migration. The return type is the new state struct which implements
            // AppState and defines the associated Event type.
            #event_registration

            // Define the inner migration logic with the original signature
            #(#attrs)*
            fn __migration_logic() #return_type #block

            // Run the migration body, assign deterministic collection
            // ids, and serialise — all under storage *merge mode*.
            //
            // Migration runs independently on every node: the
            // LazyOnAccess model emits no sync delta, so each peer
            // re-derives the v2 root from its own byte-identical v1
            // state. For the roots to match (CIP Invariant I9) the
            // serialised bytes must be a pure function of the v1 input —
            // no node-local entropy. Two sources of entropy are
            // suppressed here:
            //
            //   1. Random collection ids. A collection materialised via
            //      `UnorderedMap::new()` / `Vector::new()` (or `.into()`
            //      on such a type) gets an `Id::random()` at
            //      construction. `__assign_deterministic_ids()` re-keys
            //      every top-level collection field to its
            //      `compute_collection_id(None, field)` id (and re-keys
            //      `Vector` elements by index), mirroring the
            //      `#[app::init]` wrapper.
            //
            //   2. Node-local CRDT metadata. `LwwRegister::new(...)`
            //      (reached via `.into()`, e.g. `total: count.into()`, or
            //      as map/vector values) stamps `env::hlc_timestamp()` +
            //      `env::executor_id()` into the *value* unless merge mode
            //      is active; the same applies to `Element` update
            //      timestamps. Merge mode forces the deterministic zero
            //      stamp instead, exactly as `merge_root_state()` does for
            //      the CRDT merge path. Without it two nodes bake their
            //      own node_id/timestamp into the v2 root and diverge even
            //      though the logical state is identical — this is what
            //      the `invariant-reshuffle` scenario exercises.
            let output_bytes = ::calimero_storage::env::with_merge_mode(|| {
                let mut new_state = __migration_logic();
                new_state.__assign_deterministic_ids();
                ::calimero_sdk::borsh::to_vec(&new_state)
            });

            // Serialize the new state
            let output_bytes = match output_bytes {
                Ok(b) => b,
                Err(e) => {
                    ::calimero_sdk::env::panic_str(
                        &::std::format!("Migration serialization failed: {:?}", e)
                    );
                }
            };

            // Return the serialized state to the runtime
            ::calimero_sdk::env::value_return(&Ok::<Vec<u8>, Vec<u8>>(output_bytes));
        }

        /// Native version of the migration function for testing.
        #[cfg(not(target_arch = "wasm32"))]
        #(#attrs)*
        #vis #sig #block
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::TokenStream;
    use quote::quote;

    #[test]
    fn migrate_expansion_produces_wasm_export() {
        let input = quote! {
            fn migrate_v1_to_v2() -> Vec<u8> {
                vec![1, 2, 3]
            }
        };
        let output = migrate_impl(TokenStream::new(), input);
        let expanded = output.to_string();

        assert!(
            expanded.contains("extern \"C\""),
            "expected WASM extern \"C\" export in expansion: {}",
            expanded
        );
        assert!(
            expanded.contains("value_return"),
            "expected value_return call in expansion: {}",
            expanded
        );
        assert!(
            expanded.contains("__migration_logic"),
            "expected inner __migration_logic in expansion: {}",
            expanded
        );
        assert!(
            expanded.contains("no_mangle"),
            "expected #[no_mangle] in expansion: {}",
            expanded
        );
        assert!(
            expanded.contains("event :: register"),
            "expected event::register call in expansion: {}",
            expanded
        );
        assert!(
            expanded.contains("__assign_deterministic_ids"),
            "expected __assign_deterministic_ids call in expansion (CIP I9 cross-node \
             determinism for migrate-created collections): {}",
            expanded
        );
        assert!(
            expanded.contains("with_merge_mode"),
            "expected with_merge_mode wrap in expansion (CIP I9 cross-node determinism: \
             suppresses LwwRegister/Element node-local timestamps during migrate): {}",
            expanded
        );
    }

    #[test]
    fn migrate_expansion_preserves_native_stub() {
        let input = quote! {
            pub fn my_migrate() -> Vec<u8> {
                vec![]
            }
        };
        let output = migrate_impl(TokenStream::new(), input);
        let expanded = output.to_string();

        assert!(
            expanded.contains("my_migrate"),
            "expected function name in expansion: {}",
            expanded
        );
        assert!(
            expanded.contains("not (target_arch = \"wasm32\")")
                || expanded.contains("not(target_arch = \"wasm32\")"),
            "expected native cfg stub in expansion: {}",
            expanded
        );
    }
}
