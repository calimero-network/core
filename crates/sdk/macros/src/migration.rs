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

    // A migrate returns either the new state `S` or a `(S, W)` tuple, where `W`
    // is a transient migration witness emitted for `#[app::migration_check]`
    // (carried on the Outcome, never persisted). `state_ty` is the AppState
    // type used for event/schema registration; `is_witness` gates the emit.
    let (state_ty, is_witness): (Option<syn::Type>, bool) = match return_type {
        ReturnType::Type(_, ty) => match &**ty {
            syn::Type::Tuple(t) if t.elems.len() == 2 => (Some(t.elems[0].clone()), true),
            other => (Some(other.clone()), false),
        },
        ReturnType::Default => (None, false),
    };

    // Register the new state's Event emitter + SCHEMA_VERSION (PR-6c: so the
    // type-erased `app::schema_version()` reflects the migrated target on a
    // real node, not the unversioned 0).
    let event_registration = match &state_ty {
        Some(ty) => quote! {
            ::calimero_sdk::event::register::<#ty>();
            ::calimero_sdk::app::register_schema_version::<#ty>();
        },
        None => quote! {},
    };

    // Inside merge mode: bind the migrate output, assign deterministic ids to
    // the state, and serialise the state (+ optional witness) to bytes. Yields
    // `Result<(state_bytes, Option<witness_bytes>), borsh::io::Error>`.
    let bind_and_serialize = if is_witness {
        quote! {
            let (mut __new_state, __witness) = __migration_logic();
            __new_state.__assign_deterministic_ids();
            let __state_bytes = ::calimero_sdk::borsh::to_vec(&__new_state)?;
            let __witness_bytes = ::calimero_sdk::borsh::to_vec(&__witness)?;
            ::core::result::Result::Ok(
                (__state_bytes, ::core::option::Option::Some(__witness_bytes))
            )
        }
    } else {
        quote! {
            let mut __new_state = __migration_logic();
            __new_state.__assign_deterministic_ids();
            let __state_bytes = ::calimero_sdk::borsh::to_vec(&__new_state)?;
            ::core::result::Result::Ok(
                (__state_bytes, ::core::option::Option::<::std::vec::Vec<u8>>::None)
            )
        }
    };

    // Only emit the witness on the side channel when the migrate actually
    // returns one (a `(State, Witness)` tuple); otherwise generate nothing so
    // the common single-return path carries no dead emit call.
    let emit_witness = if is_witness {
        quote! {
            if let ::core::option::Option::Some(__w) = __witness_opt {
                ::calimero_sdk::env::emit_migration_witness(&__w);
            }
        }
    } else {
        quote! {}
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
            // Pin the closure's error type so the `?` on `borsh::to_vec`
            // resolves unambiguously (borsh's io::Error has multiple `From`
            // targets in scope otherwise).
            let __serialized: ::core::result::Result<
                (::std::vec::Vec<u8>, ::core::option::Option<::std::vec::Vec<u8>>),
                ::calimero_sdk::borsh::io::Error,
            > = ::calimero_storage::env::with_merge_mode(|| {
                #bind_and_serialize
            });

            // Unpack the serialised state and the optional transient witness.
            let (__output_bytes, __witness_opt) = match __serialized {
                Ok(v) => v,
                Err(e) => {
                    ::calimero_sdk::env::panic_str(
                        &::std::format!("Migration serialization failed: {:?}", e)
                    );
                }
            };

            // Return the serialized state to the runtime; emit the transient
            // witness (if the migrate returned a `(State, Witness)` tuple) on
            // the Outcome side channel — delivered to migration_check, never persisted.
            ::calimero_sdk::env::value_return(&Ok::<Vec<u8>, Vec<u8>>(__output_bytes));
            #emit_witness
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
/// The user writes `fn check(old: OldTy, new: NewTy) -> bool { .. }`, optionally
/// with a third parameter that receives the migration witness recorded by
/// `#[app::migrate]` (`fn check(old, new, witness: W) -> bool`). This
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

    if typed.len() != sig.inputs.len() || !(2..=3).contains(&typed.len()) {
        return quote! {
            ::core::compile_error!(
                "calimero: #[app::migration_check] requires two or three parameters, \
                 `fn check(old: OldState, new: NewState) -> bool` — the old (v1) root \
                 and the produced new (v2) root — optionally followed by a third \
                 migration-witness parameter, `fn check(old, new, witness) -> bool`"
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

    // Optional third parameter: the transient migration witness. When present,
    // decode it from the witness slot of the repacked check input; panic with a
    // clear message if the migrate emitted none.
    let witness_decode = match typed.get(2) {
        Some(witness_arg) => {
            let wit_pat = match &*witness_arg.pat {
                Pat::Ident(p) => p.ident.clone(),
                _ => {
                    return quote! {
                        ::core::compile_error!(
                            "calimero: #[app::migration_check]'s third parameter must be a plain \
                             identifier, e.g. `witness: MigrationWitness`"
                        );
                    };
                }
            };
            let wit_ty = &witness_arg.ty;
            quote! {
                let #wit_pat: #wit_ty = match __witness_opt {
                    ::core::option::Option::Some(__wb) => {
                        match ::calimero_sdk::borsh::from_slice(&__wb) {
                            Ok(v) => v,
                            Err(e) => ::calimero_sdk::env::panic_str(
                                &::std::format!("migration_check: failed to deserialize witness: {:?}", e)
                            ),
                        }
                    }
                    ::core::option::Option::None => ::calimero_sdk::env::panic_str(
                        "migration_check: this check declares a witness parameter, but the \
                         migrate emitted none — return a `(State, Witness)` tuple from #[app::migrate]"
                    ),
                };
            }
        }
        None => quote! {},
    };

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

            // The produced NEW v2 root + optional transient witness arrive as the
            // runtime input, borsh-packed as `(new_state_bytes, Option<witness_bytes>)`
            // by `run_migration_check`. `new_state_bytes` is the same bytes
            // `write_migration_state` would persist; the witness is never persisted.
            let __input = match ::calimero_sdk::env::input() {
                Some(b) => b,
                None => ::calimero_sdk::env::panic_str(
                    "migration_check: no input provided via env::input()"
                ),
            };
            let (__new_bytes, __witness_opt): (
                ::std::vec::Vec<u8>,
                ::core::option::Option<::std::vec::Vec<u8>>,
            ) = match ::calimero_sdk::borsh::from_slice(&__input) {
                Ok(v) => v,
                Err(e) => ::calimero_sdk::env::panic_str(
                    &::std::format!("migration_check: failed to deserialize check input: {:?}", e)
                ),
            };
            let #new_pat: #new_ty = match ::calimero_sdk::borsh::from_slice(&__new_bytes) {
                Ok(v) => v,
                Err(e) => ::calimero_sdk::env::panic_str(
                    &::std::format!("migration_check: failed to deserialize new state: {:?}", e)
                ),
            };
            #witness_decode

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

    #[test]
    fn migrate_tuple_return_emits_witness() {
        let input = quote! {
            fn migrate() -> (V2, MigrationWitness) {
                (V2::default(), MigrationWitness { v1_count: 3 })
            }
        };
        let expanded = migrate_impl(TokenStream::new(), input).to_string();

        assert!(
            expanded.contains("emit_migration_witness"),
            "tuple return must emit the transient witness: {}",
            expanded
        );
        // Registration uses the FIRST tuple element (the new state type), not the tuple.
        assert!(
            expanded.contains("register :: < V2 >"),
            "event/schema registration must target the state type V2: {}",
            expanded
        );
        assert!(
            expanded.contains("__assign_deterministic_ids"),
            "state still gets deterministic ids under merge mode: {}",
            expanded
        );
    }

    #[test]
    fn migrate_single_return_no_witness() {
        let input = quote! {
            fn migrate() -> V2 { V2::default() }
        };
        let expanded = migrate_impl(TokenStream::new(), input).to_string();

        assert!(
            !expanded.contains("emit_migration_witness"),
            "a non-tuple return must NOT emit a witness: {}",
            expanded
        );
        assert!(
            expanded.contains("value_return"),
            "single return still produces the committed state via value_return: {}",
            expanded
        );
    }

    #[test]
    fn migration_check_three_args_decodes_witness() {
        let input = quote! {
            fn check(old: V1, new: V2, witness: MigrationWitness) -> bool {
                new.len() as u64 == witness.v1_count
            }
        };
        let expanded = migration_check_impl(TokenStream::new(), input).to_string();

        // Input is decoded as the (new_bytes, Option<witness_bytes>) tuple.
        assert!(
            expanded.contains("__witness_opt"),
            "check input must be decoded as a (new, witness) tuple: {}",
            expanded
        );
        // The witness param is bound (panics if the migrate emitted none).
        assert!(
            expanded.contains("declares a witness parameter"),
            "3-arg check must bind the witness and guard its absence: {}",
            expanded
        );
    }

    #[test]
    fn migration_check_two_args_still_supported() {
        let input = quote! {
            fn check(old: V1, new: V2) -> bool { true }
        };
        let expanded = migration_check_impl(TokenStream::new(), input).to_string();

        assert!(
            !expanded.contains("compile_error"),
            "the 2-arg form must remain valid: {}",
            expanded
        );
        // Still decodes the tuple input shape (the witness slot is simply ignored).
        assert!(
            expanded.contains("__witness_opt"),
            "2-arg check still decodes the repacked tuple input: {}",
            expanded
        );
        assert!(
            !expanded.contains("declares a witness parameter"),
            "2-arg check must NOT bind a witness parameter: {}",
            expanded
        );
    }
}
