use proc_macro2::TokenStream;
use quote::quote;
use syn::ItemFn;

pub fn migrate_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // NOTE: we use `syn::parse2()` instead of `parse_macro_input!()`
    // as the last one accepts only `proc_macro::TokenStream`,
    // but we need to work with `proc_macro2::TokenStream` here.
    let input = match syn::parse2::<ItemFn>(item) {
        Ok(i) => i,
        Err(e) => return e.to_compile_error(),
    };

    let fn_name = &input.sig.ident;
    let block = &input.block;
    let vis = &input.vis;
    let sig = &input.sig;
    let attrs = &input.attrs;

    // We define an inner function to hold the user's logic.
    // The outer function acts as the WASM export function.
    let expanded = quote! {
        #(#attrs)*
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn #fn_name() {
            // Setup panic hook for better debugging
            ::calimero_sdk::env::setup_panic_hook();

            // Define the inner user logic
            fn migration_logic() #sig {
                #block
            }

            // Execute user logic. We expect that the user's migration function returns
            // the App State struct of the new version.
            let new_state = migration_logic();

            // Serialize the new state.
            let output_bytes = match ::calimero_sdk::borsh::to_vec(&new_state) {
                Ok(b) => b,
                Err(e) => ::calimero_sdk::env::panic_str(&format!("Migration serialization failed: {:?}", e)),
            };

            // Return the serialized bytes to the Node.
            // The Node's 'update_application' handler will intercept this
            // and write it directly to the state storage key.
            ::calimero_sdk::env::value_return(&Ok::<Vec<u8>, Vec<u8>>(output_bytes));
        }

        // Add the original function to use for unit testing (non-WASM builds).
        #[cfg(not(target_arch = "wasm32"))]
        #vis fn #fn_name() #sig {
            #block
        }
    };

    TokenStream::from(expanded)
}
