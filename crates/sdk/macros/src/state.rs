use proc_macro2::{Span, TokenStream};
use quote::{quote, ToTokens};
use syn::parse::{Parse, ParseStream};
use syn::{
    parse2, BoundLifetimes, Error as SynError, GenericParam, Generics, Ident, Lifetime,
    LifetimeParam, Result as SynResult, Token, Type,
};

use crate::errors::{Errors, ParseError, Pretty};
use crate::forbidden_types::validate_fields;
use crate::items::StructOrEnumItem;
use crate::macros::infallible;
use crate::reserved::idents;
use crate::sanitizer::{Action, Case, Func, Sanitizer};

#[derive(Clone, Copy)]
pub struct StateImpl<'a> {
    ident: &'a Ident,
    generics: &'a Generics,
    emits: &'a Option<MaybeBoundEvent>,
    orig: &'a StructOrEnumItem,
}

impl ToTokens for StateImpl<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let StateImpl {
            ident,
            generics,
            emits,
            orig,
        } = *self;

        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        let mut lifetime = quote! { 'a };
        let mut event = quote! { ::calimero_sdk::event::NoEvent };

        if let Some(emits) = emits {
            if let Some(lt) = &emits.lifetime {
                lifetime = quote! { #lt };
            }
            event = {
                let event = &emits.ty;
                quote! { #event }
            };
        }

        // Generate Mergeable implementation
        let merge_impl = generate_mergeable_impl(ident, generics, orig);

        // Generate registration hook
        let registration_hook = generate_registration_hook(ident, &ty_generics);

        // Generate deterministic ID assignment method
        let assign_ids_impl = generate_assign_deterministic_ids_impl(ident, generics, orig);

        quote! {
            #orig

            impl #impl_generics ::calimero_sdk::state::AppState for #ident #ty_generics #where_clause {
                type Event<#lifetime> = #event;
            }

            // Auto-generated CRDT merge support
            #merge_impl

            // Auto-generated registration hook
            #registration_hook

            // Auto-generated deterministic ID assignment
            #assign_ids_impl
        }
        .to_tokens(tokens);
    }
}

struct MaybeBoundEvent {
    lifetime: Option<Lifetime>,
    ty: Type,
}

// todo! move all errors to ParseError

impl Parse for MaybeBoundEvent {
    #[expect(
        clippy::unwrap_in_result,
        reason = "Error handling verified - errors exist when !fine"
    )]
    fn parse(input: ParseStream<'_>) -> SynResult<Self> {
        let mut lifetime = None;

        let errors = Errors::default();

        'bounds: {
            if input.peek(Token![for]) {
                let bounds = match input.parse::<BoundLifetimes>() {
                    Ok(bounds) => bounds,
                    Err(err) => {
                        errors.subsume(err);
                        break 'bounds;
                    }
                };

                let fine = if bounds.lifetimes.is_empty() {
                    errors.subsume(SynError::new_spanned(
                        bounds.gt_token,
                        "non-empty lifetime bounds expected",
                    ));
                    false
                } else if input.is_empty() {
                    errors.subsume(SynError::new_spanned(
                        &bounds,
                        "expected an event type to immediately follow",
                    ));
                    false
                } else {
                    true
                };

                if !fine {
                    return Err(errors.take().expect("not fine, so we must have errors"));
                }

                for param in bounds.lifetimes {
                    if let GenericParam::Lifetime(LifetimeParam { lifetime: lt, .. }) = param {
                        if lifetime.is_some() {
                            errors.subsume(SynError::new(
                                lt.span(),
                                "only one lifetime can be specified",
                            ));

                            continue;
                        }
                        lifetime = Some(lt);
                    }
                }
            }
        }

        let ty = match input.parse::<Type>() {
            Ok(ty) => ty,
            Err(err) => return Err(errors.subsumed(err)),
        };

        let mut sanitizer = match parse2::<Sanitizer<'_>>(ty.to_token_stream()) {
            Ok(sanitizer) => sanitizer,
            Err(err) => return Err(errors.subsumed(err)),
        };

        let mut cases = vec![];

        if let Some(lt) = &lifetime {
            cases.push((Case::Lifetime(Some(lt)), Action::Ignore));
        }

        let mut unexpected_lifetime = |span: Span| {
            let lifetime = span
                .source_text()
                .unwrap_or_else(|| "'{lifetime}".to_owned());

            // todo! source text is unreliable
            let error = if matches!(lifetime.as_str(), "&" | "'_") {
                ParseError::MustSpecifyLifetime
            } else {
                ParseError::UseOfUndeclaredLifetime {
                    append: format!(
                        "\n\nuse the `for<{}> {}` directive to declare it",
                        lifetime,
                        Pretty::Type(&ty)
                    ),
                }
            };

            Action::Forbid(error)
        };

        let static_lifetime = Lifetime::new("'static", Span::call_site());

        cases.extend([
            (Case::Lifetime(Some(&static_lifetime)), Action::Ignore),
            (
                Case::Lifetime(None),
                Action::Custom(Func::new(&mut unexpected_lifetime)),
            ),
        ]);

        let mut outcome = sanitizer.sanitize(&cases);

        if let Some(lifetime) = &lifetime {
            if 0 == outcome.count(&Case::Lifetime(Some(lifetime)))
                && !(lifetime == &static_lifetime
                    || matches!(lifetime.ident.to_string().as_str(), "_"))
            {
                outcome
                    .errors()
                    .subsume(SynError::new(lifetime.span(), "unused lifetime specified"));
            }
        }

        outcome.check()?;

        let ty = infallible!({ parse2(sanitizer.into_token_stream()) });

        input
            .is_empty()
            .then(|| Self { lifetime, ty })
            .ok_or_else(|| input.error("unexpected token"))
    }
}

pub struct StateArgs {
    emits: Option<MaybeBoundEvent>,
}

impl Parse for StateArgs {
    fn parse(input: ParseStream<'_>) -> SynResult<Self> {
        let mut emits = None;

        if !input.is_empty() {
            if !input.peek(Ident) {
                return Err(input.error("expected an identifier"));
            }

            let ident = input.parse::<Ident>()?;

            if !input.peek(Token![=]) {
                let span = if let Some((tt, _)) = input.cursor().token_tree() {
                    tt.span()
                } else {
                    ident.span()
                };
                return Err(SynError::new(
                    span,
                    format_args!("expected `=` after `{ident}`"),
                ));
            }

            let eq = input.parse::<Token![=]>()?;

            match ident.to_string().as_str() {
                "emits" => {
                    if input.is_empty() {
                        return Err(SynError::new_spanned(
                            eq,
                            "expected an event type after `=`",
                        ));
                    }
                    emits = Some(input.parse::<MaybeBoundEvent>()?);
                }
                _ => {
                    return Err(SynError::new_spanned(
                        &ident,
                        format_args!("unexpected `{ident}`"),
                    ));
                }
            }

            if !input.is_empty() {
                return Err(input.error("unexpected token"));
            }
        }

        Ok(Self { emits })
    }
}

pub struct StateImplInput<'a> {
    pub item: &'a StructOrEnumItem,
    pub args: &'a StateArgs,
}

impl<'a> TryFrom<StateImplInput<'a>> for StateImpl<'a> {
    type Error = Errors<'a, StructOrEnumItem>;

    fn try_from(input: StateImplInput<'a>) -> Result<Self, Self::Error> {
        let errors = Errors::new(input.item);

        let (ident, generics) = match input.item {
            StructOrEnumItem::Struct(item) => (&item.ident, &item.generics),
            StructOrEnumItem::Enum(item) => (&item.ident, &item.generics),
        };

        if ident == &*idents::input() {
            errors.subsume(SynError::new_spanned(ident, ParseError::UseOfReservedIdent));
        }

        for generic in &generics.params {
            match generic {
                GenericParam::Lifetime(params) => {
                    errors.subsume(SynError::new(
                        params.lifetime.span(),
                        ParseError::NoGenericLifetimeSupport,
                    ));
                }
                GenericParam::Type(params) => {
                    if params.ident == *idents::input() {
                        errors.subsume(SynError::new_spanned(
                            &params.ident,
                            ParseError::UseOfReservedIdent,
                        ));
                    }
                }
                GenericParam::Const(_) => {}
            }
        }

        match input.item {
            StructOrEnumItem::Struct(item) => {
                validate_fields(&item.fields, &errors);
            }
            StructOrEnumItem::Enum(item) => {
                for variant in &item.variants {
                    validate_fields(&variant.fields, &errors);
                }
            }
        }

        errors.check()?;

        Ok(StateImpl {
            ident,
            generics,
            emits: &input.args.emits,
            orig: input.item,
        })
    }
}

/// Generate Mergeable trait implementation for the state struct
fn generate_mergeable_impl(
    ident: &Ident,
    generics: &Generics,
    orig: &StructOrEnumItem,
) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Extract fields from the struct
    let fields = match orig {
        StructOrEnumItem::Struct(s) => &s.fields,
        StructOrEnumItem::Enum(_) => {
            // Enums don't have fields to merge
            return quote! {
                // No Mergeable impl for enums
            };
        }
    };

    // Call merge on every field. The forbidden-type lint above guarantees
    // each field is either an SDK CRDT, an `Option<T>` / `Box<T>` of one, or a
    // user struct that derives / implements Mergeable. If a user manages to
    // smuggle in a non-Mergeable type, the trait bound below produces a clean
    // compile error pointing at that field — much better than the silent skip
    // this code used to do (which lost concurrent updates with no diagnostic).
    let merge_calls: Vec<_> = fields
        .iter()
        .enumerate()
        .map(|(idx, field)| {
            if let Some(field_name) = &field.ident {
                quote! {
                    ::calimero_storage::collections::Mergeable::merge(
                        &mut self.#field_name,
                        &other.#field_name,
                    ).map_err(|e| {
                        ::calimero_storage::collections::crdt_meta::MergeError::StorageError(
                            ::std::format!(
                                "Failed to merge field '{}': {:?}",
                                ::core::stringify!(#field_name),
                                e
                            )
                        )
                    })?;
                }
            } else {
                let field_index = syn::Index::from(idx);
                quote! {
                    ::calimero_storage::collections::Mergeable::merge(
                        &mut self.#field_index,
                        &other.#field_index,
                    ).map_err(|e| {
                        ::calimero_storage::collections::crdt_meta::MergeError::StorageError(
                            ::std::format!(
                                "Failed to merge field {}: {:?}",
                                #idx,
                                e
                            )
                        )
                    })?;
                }
            }
        })
        .collect();

    quote! {
        // ============================================================================
        // AUTO-GENERATED by #[app::state] macro
        // ============================================================================
        //
        // This Mergeable implementation enables automatic conflict resolution during sync.
        //
        // When is this called?
        // - ONLY during remote synchronization (not on local operations)
        // - ONLY when root state conflicts occur (rare)
        // - NOT on every state change (local ops are O(1))
        //
        // Performance:
        // - Local ops: O(1) - this is NOT called
        // - Remote sync: O(N) where N = number of state fields
        //
        // What it does:
        // - Calls Mergeable::merge on every field. The forbidden-type lint
        //   guarantees every field implements Mergeable (CRDT collection,
        //   LwwRegister, Option/Box of same, or user struct deriving Mergeable).
        // - Recursive: each field's merge handles its own subtree.
        //
        impl #impl_generics ::calimero_storage::collections::Mergeable for #ident #ty_generics #where_clause {
            fn merge(&mut self, other: &Self)
                -> ::core::result::Result<(), ::calimero_storage::collections::crdt_meta::MergeError>
            {
                #(#merge_calls)*
                ::core::result::Result::Ok(())
            }
        }
    }
}

/// Generate the WASM exports the host + WASM runtime call for root-state CRDT merge.
///
/// Two exports are emitted:
///
/// 1. `__calimero_register_merge` — called by the WASM runtime at
///    module-load time. Populates the WASM-side `MERGE_REGISTRY`
///    static so that `Interface::save_internal` (running inside WASM
///    during normal delta apply via `__calimero_sync_next`) can
///    dispatch the typed merge. This is the WASM-internal dispatch
///    path and is load-bearing — without it, every WASM-side root
///    entity merge fails with `NoMergeFunctionRegistered`.
///
/// 2. `__calimero_merge_root_state` — called by the host (via
///    `ContextClient::merge_root_state`) when HC / LevelWise sync
///    needs to merge a root entity. The host can't dispatch the
///    typed merge itself (separate address space), so it hands the
///    bytes to this export, which deserializes as `T`, runs
///    `Mergeable::merge`, returns serialized bytes.
fn generate_registration_hook(ident: &Ident, ty_generics: &syn::TypeGenerics<'_>) -> TokenStream {
    quote! {
        // ============================================================================
        // AUTO-GENERATED WASM Export — WASM-Internal Registration Hook
        // ============================================================================
        //
        // Called by the runtime at WASM module load. Populates the
        // WASM-side `MERGE_REGISTRY` so the WASM `Interface::save_internal`
        // can dispatch root-entity merges via the app's typed
        // `Mergeable::merge`. Required for normal `__calimero_sync_next`
        // delta apply when the delta touches the root entity.
        //
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn __calimero_register_merge() {
            ::calimero_storage::register_crdt_merge::<#ident #ty_generics>();
        }

        // ============================================================================
        // AUTO-GENERATED WASM Export — Host-Initiated Root-State Merge
        // ============================================================================
        //
        // The host invokes this export whenever it needs to merge two root-state
        // byte blobs (HC sync apply, LevelWise sync apply, anywhere a sync
        // path on the host encounters root-entity divergence). The host
        // can't deserialize the app's root type — only the WASM module
        // can, since the type only exists here.
        //
        // Input: borsh-serialized `MergeRootStateRequest` via `env::input()`.
        // Output: borsh-serialized `MergeRootStateResponse` via `env::value_return()`.
        //
        // Developer impact: ZERO — the macro hides the export entirely.
        //
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn __calimero_merge_root_state() {
            ::calimero_sdk::env::setup_panic_hook();

            let Some(args) = ::calimero_sdk::env::input() else {
                ::calimero_sdk::env::panic_str(
                    "Expected MergeRootStateRequest payload for __calimero_merge_root_state",
                )
            };

            let request: ::calimero_storage::merge::MergeRootStateRequest =
                ::calimero_sdk::borsh::from_slice(&args).unwrap_or_else(|err| {
                    ::calimero_sdk::env::panic_str(&::std::format!(
                        "Failed to deserialize MergeRootStateRequest: {err}",
                    ))
                });

            let response = match ::calimero_storage::merge::merge_root_state_typed::<
                #ident #ty_generics,
            >(
                &request.existing,
                &request.incoming,
                request.existing_created_at,
                request.existing_ts,
                request.incoming_ts,
            ) {
                ::core::result::Result::Ok(bytes) => {
                    ::calimero_storage::merge::MergeRootStateResponse::Ok(bytes)
                }
                ::core::result::Result::Err(err) => {
                    ::calimero_storage::merge::MergeRootStateResponse::Err(::std::format!(
                        "{err:?}",
                    ))
                }
            };

            let serialized = ::calimero_sdk::borsh::to_vec(&response).unwrap_or_else(|err| {
                ::calimero_sdk::env::panic_str(&::std::format!(
                    "Failed to serialize MergeRootStateResponse: {err}",
                ))
            });

            // `value_return` wraps the bytes in a `Result` discriminant
            // on the wire — match the convention every other generated
            // export uses (see migration.rs:96 / method.rs:149). The
            // success / error semantics of the merge itself live inside
            // the `MergeRootStateResponse` payload, not the wire wrapper.
            ::calimero_sdk::env::value_return(&::core::result::Result::<
                ::std::vec::Vec<u8>,
                ::std::vec::Vec<u8>,
            >::Ok(serialized));
        }
    }
}

/// Generate method to assign deterministic IDs to all collection fields.
///
/// This method is called by the init wrapper to ensure all top-level collections
/// have deterministic IDs based on their field names, regardless of how they were
/// created in the user's init() function.
fn generate_assign_deterministic_ids_impl(
    ident: &Ident,
    generics: &Generics,
    orig: &StructOrEnumItem,
) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Extract fields from the struct
    let fields = match orig {
        StructOrEnumItem::Struct(s) => &s.fields,
        StructOrEnumItem::Enum(_) => {
            // Enums don't have fields
            return quote! {};
        }
    };

    // Helper function to check if a type is a collection that needs ID assignment
    fn is_collection_type(type_str: &str) -> bool {
        type_str.contains("UnorderedMap")
            // `SortedMap` is NOT a substring of any other entry, so it must be
            // listed explicitly — otherwise a top-level `SortedMap` state field
            // keeps its `Id::random()` and diverges across nodes (CIP I9).
            || type_str.contains("SortedMap")
            || type_str.contains("Vector")
            || type_str.contains("UnorderedSet")
            || type_str.contains("Counter")
            || type_str.contains("ReplicatedGrowableArray")
            || type_str.contains("UserStorage")
            || type_str.contains("FrozenStorage")
            || type_str.contains("SharedStorage")
            // `AuthoredVector` is already matched by the `"Vector"` substring above;
            // `AuthoredMap` is NOT a substring of any entry, so it must be listed
            // explicitly or its outer wrapper id stays `Id::random()` and a
            // freshly-constructed map (in `init`/`migrate`) diverges across nodes.
            // Safe: the inner map is built with a deterministic id, so its
            // `reassign` is an idempotent no-op (no clear+reinsert), and only the
            // wrapper's id is canonicalised — owner stamps are preserved.
            || type_str.contains("AuthoredMap")
    }

    // Generate reassign calls for each collection field
    let reassign_calls: Vec<_> = fields
        .iter()
        .enumerate()
        .filter_map(|(idx, field)| {
            let field_type = &field.ty;
            let type_str = quote! { #field_type }.to_string();

            if !is_collection_type(&type_str) {
                return None;
            }

            // Handle both named fields and tuple struct fields
            if let Some(field_name) = &field.ident {
                // Named field: use field name for both access and ID
                let field_name_str = field_name.to_string();
                Some(quote! {
                    self.#field_name.reassign_deterministic_id(#field_name_str);
                })
            } else {
                // Tuple struct field: use index for access, index string for ID
                let field_index = syn::Index::from(idx);
                let field_name_str = idx.to_string();
                Some(quote! {
                    self.#field_index.reassign_deterministic_id(#field_name_str);
                })
            }
        })
        .collect();

    quote! {
        // ============================================================================
        // AUTO-GENERATED Deterministic ID Assignment
        // ============================================================================
        //
        // This method is called after init() to ensure all top-level collections have
        // deterministic IDs. This allows users to use `UnorderedMap::new()` in init()
        // while still getting deterministic IDs for proper sync behavior.
        //
        // CIP Invariant I9: Deterministic Entity IDs
        // > Given the same application code and field names, all nodes MUST generate
        // > identical entity IDs for the same logical entities.
        //
        // Note: This method is always generated (even if empty) because the init wrapper
        // unconditionally calls it. For apps without CRDT collections, this is a no-op.
        //
        impl #impl_generics #ident #ty_generics #where_clause {
            /// Assigns deterministic IDs to all collection fields based on their field names.
            ///
            /// This is called automatically by the init wrapper. Users should not call this directly.
            #[doc(hidden)]
            pub fn __assign_deterministic_ids(&mut self) {
                #(#reassign_calls)*
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    /// Helper: render the merge body for a given struct definition.
    fn render_merge(item: syn::ItemStruct) -> String {
        let ident = item.ident.clone();
        let generics = item.generics.clone();
        let orig = StructOrEnumItem::Struct(item);
        generate_mergeable_impl(&ident, &generics, &orig).to_string()
    }

    /// Regression: the generator used to skip any field whose type string
    /// didn't contain a hardcoded CRDT name (UnorderedMap, Counter, ...). User
    /// types with their own `Mergeable` impl — including those produced by
    /// `#[derive(Mergeable)]` — would silently fall through, dropping all
    /// concurrent updates to those fields with no diagnostic. Today every
    /// field gets a merge call; the trait bound enforces correctness.
    #[test]
    fn merge_impl_calls_every_field_including_user_types() {
        let item: syn::ItemStruct = parse_quote! {
            pub struct AppRoot {
                pub counter: Counter,
                pub user: UserDerivedStruct,
                pub items: UnorderedMap<String, LwwRegister<String>>,
            }
        };

        let rendered = render_merge(item);

        for field in ["counter", "user", "items"] {
            let needle = format!("self . {field}");
            assert!(
                rendered.contains(&needle),
                "expected merge call referencing `self.{field}` in:\n{rendered}",
            );
        }
        assert_eq!(
            rendered
                .matches(":: calimero_storage :: collections :: Mergeable :: merge")
                .count(),
            3,
            "expected exactly one merge call per field in:\n{rendered}",
        );
    }

    #[test]
    fn merge_impl_handles_tuple_struct_fields() {
        let item: syn::ItemStruct = parse_quote! {
            pub struct Wrap(pub Counter, pub UserDerivedStruct);
        };

        let rendered = render_merge(item);

        for index in ["self . 0", "self . 1"] {
            assert!(
                rendered.contains(index),
                "expected merge call referencing `{index}` in:\n{rendered}",
            );
        }
    }
}
