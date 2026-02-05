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
use syn::ItemFn;

/// Generates the migration function implementation.
///
/// This function transforms a user-defined migration function into:
/// - A WASM export (for `wasm32` target) that:
///   - Sets up the panic hook for better error messages
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

    quote! {
        /// WASM export for migration function.
        ///
        /// This function is called by the node runtime during application upgrades.
        /// It reads the old state, executes the migration logic, and returns the new state bytes.
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn #fn_name() {
            ::calimero_sdk::env::setup_panic_hook();

            // Define the inner migration logic with the original signature
            #(#attrs)*
            fn __migration_logic() #return_type #block

            // Execute migration and get new state
            let new_state = __migration_logic();

            // Serialize the new state
            let output_bytes = match ::calimero_sdk::borsh::to_vec(&new_state) {
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
