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
use quote::{quote, ToTokens};
use syn::{FnArg, ItemFn, Pat, ReturnType};

/// Generates the migration function implementation.
///
/// This function transforms a user-defined migration function into:
/// - A WASM export (for `wasm32` target) that:
///   - Sets up the panic hook for better error messages
///   - Registers the event emitter and the new state's schema version for the
///     new state type (so `app::emit!` and `app::schema_version()` work)
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
            // PR-6c: surface the new state's SCHEMA_VERSION so the type-erased
            // `app::schema_version()` (read at the identity-gated storage stamp
            // site) reflects the migrated target on a real node. Without this
            // the migrate entrypoint would leave it at the unversioned 0,
            // mis-stamping every converted entry.
            ::calimero_sdk::app::register_schema_version::<#ty>();
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

            // Register the event emitter and the new state's schema version for
            // the new state type so app::emit! and app::schema_version() work
            // during and after migration. The return type is the new state
            // struct which implements AppState, defines the associated Event
            // type, and declares SCHEMA_VERSION.
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

/// Generates the migration-check function implementation.
///
/// This is the sibling of [`migrate_impl`]: where `#[app::migrate]` *produces*
/// the new v2 root, `#[app::migration_check]` is a read-only predicate the
/// runtime invokes on that produced root **before** it is committed. A `false`
/// result (or a trap) lets the runtime logically abort the migration, leaving
/// the still-untouched v1 root intact.
///
/// The user writes `fn check(old: OldTy, new: NewTy) -> bool { .. }`. This
/// function transforms it into:
/// - A WASM export (for `wasm32` target) named `__calimero_migration_check`
///   that:
///   - Sets up the panic hook for better error messages
///   - Reads the OLD v1 root via `calimero_sdk::read_raw()` and borsh-decodes
///     it into the `old` parameter type (still v1 in the store, exactly as
///     `#[app::migrate]` reads it)
///   - Borsh-decodes the produced NEW root from `env::input()` into the `new`
///     parameter type (the same bytes `write_migration_state` would persist)
///   - Runs the user's predicate body
///   - Borsh-serializes the `bool` result and returns it via `value_return`
/// - The original function (for non-WASM targets) for testing
///
/// Unlike `migrate_impl` this is **not** wrapped in `with_merge_mode`: it is a
/// pure read-only predicate, never assigns deterministic ids, and produces no
/// state — so none of the cross-node determinism machinery applies.
///
/// # Arguments
///
/// * `_attr` - Attribute arguments (currently unused)
/// * `item` - The function item to transform
///
/// # Returns
///
/// A `TokenStream` containing the generated code for both WASM and non-WASM targets.
pub fn migration_check_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = match syn::parse2::<ItemFn>(item) {
        Ok(i) => i,
        Err(e) => return e.to_compile_error(),
    };

    let block = &input.block;
    let vis = &input.vis;
    let attrs = &input.attrs;
    let sig = &input.sig;

    // Require exactly two value params: `old: OldTy`, `new: NewTy`.
    let typed: Vec<_> = sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(pt) => Some(pt),
            FnArg::Receiver(_) => None,
        })
        .collect();

    if typed.len() != sig.inputs.len() || typed.len() != 2 {
        return quote! {
            ::core::compile_error!(
                "calimero: #[app::migration_check] requires exactly two parameters, \
                 `fn check(old: OldState, new: NewState) -> bool` — the old (v1) root \
                 and the produced new (v2) root"
            );
        };
    }

    // Require a concrete `-> bool` return.
    match &sig.output {
        ReturnType::Type(_, ty) => {
            if ty.to_token_stream().to_string().replace(' ', "") != "bool" {
                return quote! {
                    ::core::compile_error!(
                        "calimero: #[app::migration_check] must return `bool` — \
                         `true` to commit the migration, `false` to logically abort it"
                    );
                };
            }
        }
        ReturnType::Default => {
            return quote! {
                ::core::compile_error!(
                    "calimero: #[app::migration_check] must return `bool` — \
                     `true` to commit the migration, `false` to logically abort it"
                );
            };
        }
    }

    let old_arg = typed[0];
    let new_arg = typed[1];

    // Bind names for the user body — copy the user's own param idents so their
    // block compiles unchanged.
    let old_pat = match &*old_arg.pat {
        Pat::Ident(p) => &p.ident,
        _ => {
            return quote! {
                ::core::compile_error!(
                    "calimero: #[app::migration_check]'s first parameter must be a plain \
                     identifier, e.g. `old: OldState`"
                );
            };
        }
    };
    let new_pat = match &*new_arg.pat {
        Pat::Ident(p) => &p.ident,
        _ => {
            return quote! {
                ::core::compile_error!(
                    "calimero: #[app::migration_check]'s second parameter must be a plain \
                     identifier, e.g. `new: NewState`"
                );
            };
        }
    };
    let old_ty = &old_arg.ty;
    let new_ty = &new_arg.ty;

    quote! {
        /// WASM export for the migration-check predicate.
        ///
        /// This function is called by the node runtime on the produced v2 root
        /// *before* it is committed. It reads the old (v1) state, deserializes
        /// the produced new (v2) state from the runtime-supplied input, runs the
        /// author's predicate, and returns the borsh-serialized `bool` verdict.
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn __calimero_migration_check() {
            ::calimero_sdk::env::setup_panic_hook();

            // Read the OLD v1 root — still v1 in the store, exactly as
            // `#[app::migrate]` reads it (the v1 root is not mutated until the
            // migration commits, which has not happened yet at check time).
            let __old_bytes = match ::calimero_sdk::state::read_raw() {
                Some(b) => b,
                None => ::calimero_sdk::env::panic_str(
                    "migration_check: no old root state found via read_raw()"
                ),
            };
            let #old_pat: #old_ty = match ::calimero_sdk::borsh::from_slice(&__old_bytes) {
                Ok(v) => v,
                Err(e) => ::calimero_sdk::env::panic_str(
                    &::std::format!("migration_check: failed to deserialize old state: {:?}", e)
                ),
            };

            // The produced NEW v2 root arrives as the runtime input — the same
            // bytes `write_migration_state` would persist.
            let __new_bytes = match ::calimero_sdk::env::input() {
                Some(b) => b,
                None => ::calimero_sdk::env::panic_str(
                    "migration_check: no new state provided via env::input()"
                ),
            };
            let #new_pat: #new_ty = match ::calimero_sdk::borsh::from_slice(&__new_bytes) {
                Ok(v) => v,
                Err(e) => ::calimero_sdk::env::panic_str(
                    &::std::format!("migration_check: failed to deserialize new state: {:?}", e)
                ),
            };

            // Run the author's predicate.
            let __result: bool = (|| #block)();

            // Return the borsh-serialized verdict. Mirroring `#[app::migrate]`,
            // the raw payload handed to `value_return`'s Ok branch is what the
            // runtime extracts from `outcome.returns` — here `borsh(bool)`.
            let __verdict = match ::calimero_sdk::borsh::to_vec(&__result) {
                Ok(b) => b,
                Err(e) => ::calimero_sdk::env::panic_str(
                    &::std::format!("migration_check: failed to serialize verdict: {:?}", e)
                ),
            };
            ::calimero_sdk::env::value_return(&Ok::<Vec<u8>, Vec<u8>>(__verdict));
        }

        /// Native version of the migration-check function for testing.
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
            expanded.contains("register_schema_version"),
            "expected app::register_schema_version call in expansion (PR-6c: migrate must \
             register the new state's SCHEMA_VERSION so app::schema_version() reflects the \
             migrated target on a real node, not the unversioned 0): {}",
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
    fn migration_check_expansion_produces_wasm_export() {
        let input = quote! {
            fn check(old: AppV1, new: AppV2) -> bool {
                old.len() == new.len()
            }
        };
        let out = migration_check_impl(TokenStream::new(), input).to_string();

        assert!(
            out.contains("extern \"C\""),
            "expected WASM extern \"C\" export in expansion: {}",
            out
        );
        assert!(
            out.contains("__calimero_migration_check"),
            "expected #[no_mangle] __calimero_migration_check export name in expansion: {}",
            out
        );
        assert!(
            out.contains("read_raw"),
            "expected read_raw() to load the old v1 root in expansion: {}",
            out
        );
        assert!(
            out.contains("input"),
            "expected env::input() to load the produced v2 root bytes in expansion: {}",
            out
        );
        assert!(
            out.contains("value_return"),
            "expected value_return of the borsh Ok::<bool, _> result in expansion: {}",
            out
        );
        assert!(
            out.contains("no_mangle"),
            "expected #[no_mangle] in expansion: {}",
            out
        );
    }

    #[test]
    fn migration_check_expansion_preserves_native_stub() {
        let input = quote! {
            pub fn my_check(old: AppV1, new: AppV2) -> bool {
                true
            }
        };
        let out = migration_check_impl(TokenStream::new(), input).to_string();

        assert!(
            out.contains("my_check"),
            "expected function name in expansion: {}",
            out
        );
        assert!(
            out.contains("not (target_arch = \"wasm32\")")
                || out.contains("not(target_arch = \"wasm32\")"),
            "expected native cfg stub in expansion: {}",
            out
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
