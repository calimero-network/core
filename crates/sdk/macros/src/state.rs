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
    version: Option<u32>,
    orig: &'a StructOrEnumItem,
}

impl ToTokens for StateImpl<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let StateImpl {
            ident,
            generics,
            emits,
            version,
            orig,
        } = *self;

        // `#[app::state(version = N)]` overrides the AppState::SCHEMA_VERSION
        // default (0). This is the target the owner-driven convert +
        // migrate_my_entries() compare each identity-gated entry against.
        let schema_version_const = version.map(|v| quote! { const SCHEMA_VERSION: u32 = #v; });

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

        // Re-key registration for nested CRDT-value types (#2577): so a custom
        // struct stored as a collection value has its nested collections given
        // deterministic ids and converges instead of being LWW'd as a blob.
        // The method holds the registrations; the wasm load hook, the native
        // test bridge, and `register_crdt_merge_for_test` all call it.
        let rekey_register_method = generate_rekey_register_method(ident, generics, orig);
        let rekey_call = quote! { <#ident #ty_generics>::__calimero_register_rekey(); };

        // Generate registration hook
        let registration_hook = generate_registration_hook(ident, &ty_generics, &rekey_call);

        // Generate deterministic ID assignment method
        let assign_ids_impl = generate_assign_deterministic_ids_impl(ident, generics, orig);

        // Generate the one-tap migrate_my_entries() export over authored fields
        let migrate_my_entries_impl = generate_migrate_my_entries_impl(ident, generics, orig);

        // Generate the in-process test-harness bridge (native-only)
        let test_state_impl = generate_test_state_impl(ident, generics, orig, &rekey_call);

        quote! {
            // State is always persisted via borsh (init save, root-state merge,
            // `merge_root_state_typed::<T>`), so the macro injects the derives and
            // the crate redirect itself — authors no longer hand-write
            // `#[derive(BorshSerialize, BorshDeserialize)]` + `#[borsh(crate = ...)]`
            // on every state type. The full-path derive only selects the proc-macro;
            // the generated code still resolves the borsh runtime through `::borsh`
            // by default, so the `crate` attribute redirecting it to the SDK re-export
            // is load-bearing.
            #[derive(
                ::calimero_sdk::borsh::BorshSerialize,
                ::calimero_sdk::borsh::BorshDeserialize,
            )]
            #[borsh(crate = "::calimero_sdk::borsh")]
            #orig

            impl #impl_generics ::calimero_sdk::state::AppState for #ident #ty_generics #where_clause {
                type Event<#lifetime> = #event;
                #schema_version_const
            }

            // Auto-generated CRDT merge support
            #merge_impl

            // Auto-generated registration hook
            #registration_hook

            // Auto-generated nested-value re-key registration (#2577)
            #rekey_register_method

            // Auto-generated deterministic ID assignment
            #assign_ids_impl

            // Auto-generated one-tap authored-data migration export
            #migrate_my_entries_impl

            // Auto-generated TestHost bridge
            #test_state_impl
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
    /// The `AppState::SCHEMA_VERSION` this binary targets, from
    /// `#[app::state(version = N)]`. `None` ⇒ unversioned (defaults to 0).
    /// The owner-driven convert + `migrate_my_entries()` compare against this,
    /// so a v2 binary that omits it would never convert its identity-gated data.
    version: Option<u32>,
}

impl Parse for StateArgs {
    fn parse(input: ParseStream<'_>) -> SynResult<Self> {
        let mut emits = None;
        let mut version = None;

        // Comma-separated `key = value` pairs. `emits` consumes the rest of the
        // stream (its event type may itself contain commas, e.g. generics), so
        // it must come last; `version = N` may precede it or stand alone.
        while !input.is_empty() {
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
                    // Consumes the remainder of the args (must be the last key).
                    emits = Some(input.parse::<MaybeBoundEvent>()?);
                    break;
                }
                "version" => {
                    if input.is_empty() {
                        return Err(SynError::new_spanned(
                            eq,
                            "expected an integer schema version after `=`",
                        ));
                    }
                    version = Some(input.parse::<syn::LitInt>()?.base10_parse::<u32>()?);
                }
                _ => {
                    return Err(SynError::new_spanned(
                        &ident,
                        format_args!("unexpected `{ident}`"),
                    ));
                }
            }

            if input.peek(Token![,]) {
                let _: Token![,] = input.parse()?;
            } else if !input.is_empty() {
                return Err(input.error("expected `,` between `#[app::state]` arguments"));
            }
        }

        Ok(Self { emits, version })
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

        // `#[app::state]` injects the borsh derives + `#[borsh(crate = ...)]`
        // itself. A leftover manual `BorshSerialize`/`BorshDeserialize` derive, or
        // a `#[borsh(crate = ...)]` redirect, would otherwise collide with the
        // injected one and surface as a cryptic "conflicting implementations of
        // trait `BorshSerialize`" error pointing at generated code. Catch those
        // here and point straight at the attribute to delete.
        //
        // Only the `crate` key collides — other container-level borsh keys
        // (`#[borsh(init = ...)]` on a struct, `#[borsh(use_discriminant = ...)]`
        // on an enum) are legitimate and the macro does not supply them, so they
        // must pass through. Flagging every `#[borsh(...)]` would make those
        // attributes impossible to use on a state type.
        let attrs = match input.item {
            StructOrEnumItem::Struct(item) => &item.attrs,
            StructOrEnumItem::Enum(item) => &item.attrs,
        };

        for attr in attrs {
            if attr.path().is_ident("borsh") {
                let mut sets_crate = false;
                if let Err(err) = attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("crate") {
                        sets_crate = true;
                    }
                    // Consume `= <value>` so iteration reaches every key rather
                    // than stopping at the first `key = value` pair.
                    if meta.input.peek(Token![=]) {
                        let _: TokenStream = meta.value()?.parse()?;
                    }
                    Ok(())
                }) {
                    errors.subsume(err);
                }

                if sets_crate {
                    errors.subsume(SynError::new_spanned(
                        attr,
                        "remove `crate = ...` from this `#[borsh(...)]`: `#[app::state]` injects the borsh crate redirect itself",
                    ));
                }
            } else if attr.path().is_ident("derive") {
                if let Err(err) = attr.parse_nested_meta(|meta| {
                    if meta.path.segments.last().is_some_and(|seg| {
                        matches!(
                            seg.ident.to_string().as_str(),
                            "BorshSerialize" | "BorshDeserialize"
                        )
                    }) {
                        errors.subsume(SynError::new_spanned(
                            &meta.path,
                            "remove this derive: `#[app::state]` now injects `BorshSerialize` and `BorshDeserialize`",
                        ));
                    }
                    Ok(())
                }) {
                    errors.subsume(err);
                }
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
            version: input.args.version,
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
fn generate_registration_hook(
    ident: &Ident,
    ty_generics: &syn::TypeGenerics<'_>,
    rekey_call: &TokenStream,
) -> TokenStream {
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
            // Register nested CRDT-value-type re-key thunks (#2577) so struct
            // values converge instead of being last-writer-wins'd as a blob.
            #rekey_call
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

/// Generate the [`calimero_sdk::testing::TestState`] bridge for the state type.
///
/// `TestHost` lives in `calimero_sdk` but can't name
/// `calimero_storage::collections::Root` (the storage crate depends on the SDK,
/// not vice-versa). This generated impl — emitted into the app crate, which
/// depends on both — closes that loop: it drives `Root` for install / load /
/// mutate / commit against the native mock store so app methods run under a
/// plain `cargo test`.
///
/// Gated on `#[cfg(test)]` so it's absent from normal / wasm builds. It reaches
/// the CRDT merge registry via `register_crdt_merge_for_test` — an always-native
/// wrapper that's a no-op unless `calimero-storage`'s `testing` feature compiles
/// the registry in. That keeps `cargo test` compiling for every example app,
/// even ones with no `TestHost` tests of their own; an app that actually drives
/// `TestHost` enables the feature (as a dev-dependency) so real registration
/// happens and `Root` writes don't hit `NoMergeFunctionRegistered`.
///
/// Only emitted for struct states. Enum states have no
/// `__assign_deterministic_ids` method (no fields), and a CRDT root is always a
/// struct in practice, so an enum-root app simply has no `TestHost` bridge.
/// Generate `__calimero_register_rekey()` — registers a re-key thunk for every
/// type that appears in a state field, so a custom struct stored as a collection
/// VALUE gets its nested collections deterministically re-keyed (and thus
/// converges) instead of being last-writer-wins'd as an opaque blob (#2577).
///
/// We register EVERY type token found in each field (the collection itself, its
/// key/value types, and so on). `register_rekey_if_supported!` autoref-dispatches
/// to a real registration only for `RekeyTarget` types and is a safe no-op for
/// leaves (`String`, `u64`, `LwwRegister<_>`, …), so over-collecting (including
/// key types) is harmless.
///
/// SCOPE — one level of custom-struct nesting. This registers the value types of
/// the ROOT state's collection fields. Built-in collections self-register in
/// their constructors, so any depth of *built-in* nesting works. But a custom
/// struct reachable only through ANOTHER custom struct's collection (state →
/// `Map<_, Outer>` → `Map<_, Inner>`, where both are app structs) is not
/// registered here, so `Inner` would still be LWW'd. Deep custom nesting is the
/// follow-up; today's apps don't hit it.
fn generate_rekey_register_method(
    ident: &Ident,
    generics: &Generics,
    orig: &StructOrEnumItem,
) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let fields = match orig {
        StructOrEnumItem::Struct(s) => &s.fields,
        StructOrEnumItem::Enum(_) => return quote! {},
    };

    let calls = rekey_register_calls(fields);

    quote! {
        impl #impl_generics #ident #ty_generics #where_clause {
            /// Registers nested CRDT-value re-key thunks. Called at WASM module
            /// load and by the native test bridge; idempotent. Not for direct use.
            #[doc(hidden)]
            pub fn __calimero_register_rekey() {
                #calls
            }
        }
    }
}

/// Emit one `register_rekey_if_supported!(T)` per distinct type token reachable
/// from `fields` (each field type plus its generic args, recursively). Shared by
/// the root `#[app::state]` scan and `#[derive(Mergeable)]`'s
/// `register_nested_value_types` override, so a nested custom struct registers
/// its own collection-value types the same way the root does — turning the
/// root's one-level scan into a full walk of the reachable value graph.
///
/// Dedup is by token string (syntactic, not semantic): the same value type can
/// appear in several fields (e.g. two `UnorderedMap<String, TeamStats>`), so
/// collapse identically-written types to one call. Types written differently but
/// equal (`TeamStats` vs `crate::TeamStats`) still emit two calls — harmless:
/// they share a `TypeId`, and registration is idempotent (`or_insert`), so the
/// registry holds one entry either way.
pub(crate) fn rekey_register_calls(fields: &syn::Fields) -> TokenStream {
    let mut types: Vec<Type> = Vec::new();
    for field in fields.iter() {
        collect_type_paths(&field.ty, &mut types);
    }

    let mut seen = std::collections::HashSet::new();
    let calls = types
        .iter()
        .filter(|ty| seen.insert(ty.to_token_stream().to_string()))
        .map(|ty| {
            quote! { ::calimero_storage::register_rekey_if_supported!(#ty); }
        });

    quote! { #(#calls)* }
}

/// Recursively collect every `Type::Path` node reachable from `ty` (the type
/// itself plus its generic arguments, tuple/reference/array elements), so the
/// registration above can offer each to `register_rekey_if_supported!`.
///
/// Shared with `#[derive(Mergeable)]` (via [`rekey_register_calls`]), which runs
/// the identical per-field scan so a nested custom struct registers its own
/// value types — extending the root's one-level scan to the full graph.
pub(crate) fn collect_type_paths(ty: &Type, out: &mut Vec<Type>) {
    match ty {
        Type::Path(tp) => {
            out.push(ty.clone());
            if let Some(seg) = tp.path.segments.last() {
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    for arg in &args.args {
                        if let syn::GenericArgument::Type(inner) = arg {
                            collect_type_paths(inner, out);
                        }
                    }
                }
            }
        }
        Type::Reference(r) => collect_type_paths(&r.elem, out),
        Type::Paren(p) => collect_type_paths(&p.elem, out),
        Type::Group(g) => collect_type_paths(&g.elem, out),
        Type::Array(a) => collect_type_paths(&a.elem, out),
        Type::Tuple(t) => {
            for elem in &t.elems {
                collect_type_paths(elem, out);
            }
        }
        _ => {}
    }
}

fn generate_test_state_impl(
    ident: &Ident,
    generics: &Generics,
    orig: &StructOrEnumItem,
    rekey_call: &TokenStream,
) -> TokenStream {
    if matches!(orig, StructOrEnumItem::Enum(_)) {
        return quote! {};
    }

    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    quote! {
        #[cfg(test)]
        impl #impl_generics ::calimero_sdk::testing::TestState for #ident #ty_generics #where_clause {
            fn __test_reset() {
                ::calimero_storage::env::reset_environment();
            }

            fn __test_install(build: &mut dyn ::core::ops::FnMut() -> Self) {
                // Native equivalent of the WASM `__calimero_register_merge`
                // export: populate the CRDT merge registry so root-entity
                // writes can resolve their typed merge. Idempotent. The
                // `_for_test` wrapper is always present off-wasm (a no-op
                // unless the `testing` feature is enabled), so this bridge
                // compiles even for example apps without `TestHost` tests.
                ::calimero_storage::register_crdt_merge_for_test::<#ident #ty_generics>();
                // Register nested CRDT-value-type re-key thunks (#2577).
                #rekey_call

                let root = ::calimero_storage::collections::Root::new(|| {
                    let mut state = build();
                    // Mirror the `#[app::init]` entrypoint: deterministic IDs
                    // then a single commit of the freshly-built root.
                    state.__assign_deterministic_ids();
                    state
                });
                root.commit();
            }

            fn __test_with_mut(f: &mut dyn ::core::ops::FnMut(&mut Self)) {
                let mut app = ::calimero_storage::collections::Root::<#ident #ty_generics>::fetch()
                    .expect("TestHost: app state has not been initialized");
                // Going through DerefMut marks the root dirty so `commit`
                // persists the mutation.
                f(&mut *app);
                app.commit();
            }

            fn __test_with_ref(f: &mut dyn ::core::ops::FnMut(&Self)) {
                let app = ::calimero_storage::collections::Root::<#ident #ty_generics>::fetch()
                    .expect("TestHost: app state has not been initialized");
                f(&*app);
            }

            fn __test_with_executor(id: [u8; 32], f: &mut dyn ::core::ops::FnMut()) {
                ::calimero_storage::env::with_executor_id(id, || f());
            }

            fn __test_mirror_root() {
                // Application state lives in `calimero_storage`'s native mock,
                // but `calimero_sdk::read_raw()` reads a *separate* SDK host
                // map. Mirror the committed root `Entry` across so `read_raw()`
                // (and therefore a `#[app::migrate]` body) observes the
                // committed state. No-op until something is committed.
                if let ::core::option::Option::Some(__root) =
                    ::calimero_storage::env::read_committed_root_entry()
                {
                    ::calimero_sdk::env::__test_seed_root(__root);
                }
            }

            fn __test_install_migrated(build: &mut dyn ::core::ops::FnMut() -> Self) {
                ::calimero_storage::register_crdt_merge_for_test::<#ident #ty_generics>();
                #rekey_call
                // Faithfully mirror the WASM `#[app::migrate]` export: run the
                // migrate body and deterministic-id assignment under storage
                // *merge mode* so any `LwwRegister`/`Element` stamped inside the
                // body is zeroed and the migrated root is byte-identical across
                // nodes — the property a determinism test must exercise.
                ::calimero_storage::env::with_merge_mode(|| {
                    let root = ::calimero_storage::collections::Root::new(|| {
                        let mut state = build();
                        state.__assign_deterministic_ids();
                        state
                    });
                    root.commit();
                });
            }

            fn __test_root_hash() -> ::core::option::Option<[u8; 32]> {
                // The merkle root recorded by the most recent commit. It folds in
                // every child-collection entry's hash, so comparing it across two
                // runs detects divergence *inside* carried/seeded collections —
                // not just in the top-level root struct.
                ::calimero_storage::env::root_hash()
            }

            fn __test_with_mut_merged(f: &mut dyn ::core::ops::FnMut(&mut Self)) {
                // Like `__test_with_mut`, but under storage *merge mode* — the
                // way `__calimero_sync_next` applies an inbound delta. Stamps are
                // zeroed so the mutation is byte-identical across nodes, modelling
                // an absorbed delta's verbatim replay.
                ::calimero_storage::env::with_merge_mode(|| {
                    let mut app = ::calimero_storage::collections::Root::<#ident #ty_generics>::fetch()
                        .expect("TestHost: app state has not been initialized");
                    f(&mut *app);
                    app.commit();
                });
            }
        }
    }
}

/// Whether a field type is a single-owner identity-gated collection that the
/// one-tap `migrate_my_entries()` sweeps. Matches `AuthoredMap`/`AuthoredVector`
/// only. `SharedStorage` (group writer-set) is deliberately excluded: its value
/// converts via the organic writer-write substrate, and a batch re-write would
/// force a `T: Clone` bound on every shared value type.
fn is_identity_gated_collection(type_str: &str) -> bool {
    type_str.contains("AuthoredMap") || type_str.contains("AuthoredVector")
}

/// Generate the one-tap `migrate_my_entries()` wasm export + its inherent
/// helper. For each declared `AuthoredMap`/`AuthoredVector` field, re-write
/// every entry the caller owns that is still below the target schema version,
/// routing through the owner-driven convert (which re-stamps + re-signs). Plain
/// (convergent) collections are skipped — they migrate via the whole-root
/// rebuild. Emitted only when the state has at least one authored field.
fn generate_migrate_my_entries_impl(
    ident: &Ident,
    generics: &Generics,
    orig: &StructOrEnumItem,
) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let fields = match orig {
        StructOrEnumItem::Struct(s) => &s.fields,
        StructOrEnumItem::Enum(_) => return quote! {},
    };

    let mut field_loops: Vec<TokenStream> = Vec::new();
    for (idx, field) in fields.iter().enumerate() {
        let field_type = &field.ty;
        let type_str = quote! { #field_type }.to_string();
        if !is_identity_gated_collection(&type_str) {
            continue;
        }

        let access = if let Some(name) = &field.ident {
            quote! { self.#name }
        } else {
            let index = syn::Index::from(idx);
            quote! { self.#index }
        };

        // `AuthoredVector` also contains the substring "Vector"; check the map
        // first, then fall to the vector shape (keyed-by-index vs keyed-by-key).
        let loop_body = if type_str.contains("AuthoredMap") {
            quote! {
                // Collect into an owned Vec in its own statement so the immutable
                // `entries()` borrow is fully released before the mutable owner
                // re-write below (an `if let` would extend it across the block).
                let __entries: ::std::vec::Vec<_> = match #access.entries() {
                    ::core::result::Result::Ok(__it) => __it.collect(),
                    ::core::result::Result::Err(_) => ::std::vec::Vec::new(),
                };
                for (__k, __v) in __entries {
                    let __owned = #access.owned_by_me(&__k).unwrap_or(false);
                    let __stale =
                        #access.entry_schema_version(&__k).ok().flatten().unwrap_or(0) < __target;
                    if __owned && __stale {
                        match #access.update(&__k, __v) {
                            ::core::result::Result::Ok(()) => {
                                __converted = __converted.saturating_add(1);
                            }
                            ::core::result::Result::Err(_) => {
                                __remaining = __remaining.saturating_add(1);
                            }
                        }
                    }
                }
            }
        } else {
            quote! {
                let __len = #access.len().unwrap_or(0);
                for __i in 0..__len {
                    let __owned = #access.owned_by_me(__i).unwrap_or(false);
                    let __stale =
                        #access.entry_schema_version(__i).ok().flatten().unwrap_or(0) < __target;
                    if __owned && __stale {
                        // Read into an owned Option in its own statement, same
                        // borrow-release reason as the map arm above.
                        let __val = #access.get(__i).ok().flatten();
                        if let ::core::option::Option::Some(__v) = __val {
                            match #access.update(__i, __v) {
                                ::core::result::Result::Ok(()) => {
                                    __converted = __converted.saturating_add(1);
                                }
                                ::core::result::Result::Err(_) => {
                                    __remaining = __remaining.saturating_add(1);
                                }
                            }
                        }
                    }
                }
            }
        };
        field_loops.push(loop_body);
    }

    // No authored fields → nothing to migrate; emit nothing.
    if field_loops.is_empty() {
        return quote! {};
    }

    quote! {
        impl #impl_generics #ident #ty_generics #where_clause {
            /// Convert all of the caller's still-stale identity-gated entries to
            /// the target schema in one pass. Each re-write rides the owner's
            /// monotonic nonce (owner-driven convert), so it replicates as a
            /// fresh signed delta. Idempotent: already-current entries are skipped.
            #[doc(hidden)]
            pub fn __calimero_migrate_my_entries(&mut self) -> ::calimero_sdk::MigrateMyEntriesSummary {
                let __target = ::calimero_sdk::app::schema_version();
                let mut __converted: u32 = 0;
                let mut __remaining: u32 = 0;
                #(#field_loops)*
                ::calimero_sdk::MigrateMyEntriesSummary {
                    converted: __converted,
                    remaining: __remaining,
                }
            }
        }

        // One signed RPC call (`app_call "migrate_my_entries"`) converts the
        // caller's authored entries and returns `{converted, remaining}` as JSON.
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn migrate_my_entries() {
            ::calimero_sdk::env::setup_panic_hook();
            ::calimero_sdk::env::init_logging();
            ::calimero_sdk::event::register::<#ident #ty_generics>();
            ::calimero_sdk::app::register_schema_version::<#ident #ty_generics>();

            let ::core::option::Option::Some(mut app) =
                ::calimero_storage::collections::Root::<#ident #ty_generics>::fetch()
            else {
                ::calimero_sdk::env::panic_str("Failed to find or read app state")
            };
            let __summary = app.__calimero_migrate_my_entries();
            let __out = {
                #[allow(unused_imports)]
                use ::calimero_sdk::__private::IntoResult;
                match ::calimero_sdk::__private::WrappedReturn::new(__summary)
                    .into_result()
                    .to_json()
                {
                    ::core::result::Result::Ok(__o) => __o,
                    ::core::result::Result::Err(__e) => ::calimero_sdk::env::panic_str(
                        &format!("Failed to serialize migrate_my_entries output: {:?}", __e)
                    ),
                }
            };
            ::calimero_sdk::env::value_return(&__out);
            app.commit();
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
            // `SortedSet` is NOT a substring of any other entry (same reason as
            // `SortedMap`): list it explicitly or its id stays random and diverges.
            || type_str.contains("SortedSet")
            || type_str.contains("Counter")
            || type_str.contains("ReplicatedGrowableArray")
            || type_str.contains("UserStorage")
            || type_str.contains("FrozenStorage")
            || type_str.contains("SharedStorage")
            // `PermissionedStorage` and its `Ownable` alias wrap a
            // `SharedStorage`; their `reassign_deterministic_id` delegates to it,
            // so the inner wrapper gets the field-derived id and converges. Both
            // must be listed: a field written as `Ownable<_>` shows the `Ownable`
            // token (alias is not resolved here), and `PermissionedStorage` is not
            // a substring of `SharedStorage`.
            || type_str.contains("PermissionedStorage")
            || type_str.contains("Ownable")
            // `AccessControl` wraps a single guarded storage; its
            // `reassign_deterministic_id` delegates to it. The macro does not
            // recurse into nested structs, so without this its inner storage
            // keeps a random id and diverges across nodes.
            || type_str.contains("AccessControl")
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

    /// Helper: render the generated `migrate_my_entries` block for a struct.
    fn render_migrate(item: syn::ItemStruct) -> String {
        let ident = item.ident.clone();
        let generics = item.generics.clone();
        let orig = StructOrEnumItem::Struct(item);
        generate_migrate_my_entries_impl(&ident, &generics, &orig).to_string()
    }

    #[test]
    fn migrate_my_entries_targets_only_identity_gated_fields() {
        let item: syn::ItemStruct = parse_quote! {
            pub struct AppRoot {
                pub notes: AuthoredMap<String, Note>,
                pub log: AuthoredVector<Entry>,
                pub counters: UnorderedMap<String, Counter>,
                pub shared: SharedStorage<Doc>,
            }
        };

        let rendered = render_migrate(item);

        // The single-owner authored fields are swept...
        assert!(
            rendered.contains("self . notes"),
            "AuthoredMap field must be converted, got:\n{rendered}",
        );
        assert!(
            rendered.contains("self . log"),
            "AuthoredVector field must be converted, got:\n{rendered}",
        );
        // ...convergent and group-shared fields are NOT.
        assert!(
            !rendered.contains("self . counters"),
            "plain UnorderedMap must NOT be touched, got:\n{rendered}",
        );
        assert!(
            !rendered.contains("self . shared"),
            "SharedStorage (group writer-set) must NOT be in the batch, got:\n{rendered}",
        );
        // The wasm export is emitted because the struct has authored fields.
        assert!(
            rendered.contains("fn migrate_my_entries"),
            "expected a migrate_my_entries wasm export, got:\n{rendered}",
        );
    }

    #[test]
    fn migrate_my_entries_retains_entitlement_and_idempotency_guards() {
        // Regression guard: the generated sweep MUST keep both gates, else it
        // would convert foreign entries (no `owned_by_me`) or re-convert already
        // migrated ones (no `< __target` skip — breaking idempotency).
        let item: syn::ItemStruct = parse_quote! {
            pub struct AppRoot {
                pub notes: AuthoredMap<String, Note>,
                pub log: AuthoredVector<Entry>,
            }
        };
        let rendered = render_migrate(item);
        assert!(
            rendered.contains("owned_by_me"),
            "must gate conversion on ownership, got:\n{rendered}",
        );
        assert!(
            rendered.contains("entry_schema_version"),
            "must read each entry's stored schema_version, got:\n{rendered}",
        );
        assert!(
            rendered.contains("< __target"),
            "must skip entries already at/above target (idempotency), got:\n{rendered}",
        );
    }

    #[test]
    fn migrate_my_entries_not_generated_without_identity_gated_fields() {
        let item: syn::ItemStruct = parse_quote! {
            pub struct PlainRoot {
                pub counters: UnorderedMap<String, Counter>,
                pub items: Vector<u64>,
            }
        };

        let rendered = render_migrate(item);

        // No authored fields → no method, no export (nothing to convert).
        assert!(
            !rendered.contains("migrate_my_entries"),
            "apps without authored data must get no migrate export, got:\n{rendered}",
        );
    }

    #[test]
    fn state_version_arg_emits_schema_version_const() {
        // `version = N` must surface as AppState::SCHEMA_VERSION = N (the convert
        // target); omitting it must leave the trait default (0) untouched.
        let with_version: StateArgs = syn::parse_quote! { version = 2 };
        assert_eq!(with_version.version, Some(2));

        let with_both: StateArgs = syn::parse_quote! { version = 3, emits = for<'a> Event<'a> };
        assert_eq!(with_both.version, Some(3));
        assert!(with_both.emits.is_some());

        let none: StateArgs = syn::parse_quote! {};
        assert_eq!(none.version, None);

        let item: syn::ItemStruct = parse_quote! { pub struct S { x: u32 } };
        let orig = StructOrEnumItem::Struct(item.clone());
        let rendered = StateImpl {
            ident: &item.ident,
            generics: &item.generics,
            emits: &None,
            version: Some(2),
            orig: &orig,
        }
        .to_token_stream()
        .to_string();
        assert!(
            rendered.contains("const SCHEMA_VERSION : u32 = 2"),
            "version=2 must emit the SCHEMA_VERSION const, got:\n{rendered}",
        );
    }

    /// Run the `#[app::state]` input validation (which includes the
    /// borsh-attribute guard) over `item`, returning whether it was accepted.
    fn state_accepts(item: StructOrEnumItem) -> bool {
        let args = StateArgs {
            emits: None,
            version: None,
        };
        let accepted = match StateImpl::try_from(StateImplInput {
            item: &item,
            args: &args,
        }) {
            Ok(_) => true,
            Err(errors) => {
                // The error accumulator panics on drop if left non-empty, so
                // drain it (this is what the real macro entry point does on the
                // error path) before reporting rejection.
                let _ = errors.to_compile_error();
                false
            }
        };
        accepted
    }

    #[test]
    fn borsh_guard_rejects_crate_redirect_only() {
        // `try_from` reads the reserved-ident table, a thread-local the proc-macro
        // entry point initializes; do the same for this unit test's thread.
        crate::reserved::init();

        // The macro injects the borsh derives + `#[borsh(crate = ...)]`, so a
        // leftover manual derive or `crate` redirect must be rejected (otherwise
        // it surfaces later as a cryptic "conflicting implementations" error).
        assert!(
            !state_accepts(StructOrEnumItem::Struct(parse_quote! {
                #[borsh(crate = "calimero_sdk::borsh")]
                pub struct S {}
            })),
            "`#[borsh(crate = ...)]` must be rejected — the macro injects it",
        );
        assert!(
            !state_accepts(StructOrEnumItem::Struct(parse_quote! {
                #[derive(BorshSerialize, BorshDeserialize)]
                pub struct S {}
            })),
            "a manual borsh derive must be rejected — the macro injects it",
        );

        // But the macro supplies ONLY `crate`. Other legitimate container-level
        // borsh keys it never sets must pass through, or they become impossible
        // to use on a state type: `init` on a struct, `use_discriminant` on an
        // enum (required by borsh for enums with explicit discriminants).
        assert!(
            state_accepts(StructOrEnumItem::Struct(parse_quote! {
                #[borsh(init = "post_load")]
                pub struct S {}
            })),
            "`#[borsh(init = ...)]` is legitimate and must pass through",
        );
        assert!(
            state_accepts(StructOrEnumItem::Enum(parse_quote! {
                #[borsh(use_discriminant = true)]
                pub enum E { A = 1, B = 2 }
            })),
            "`#[borsh(use_discriminant = ...)]` is legitimate and must pass through",
        );

        // And the common case — no item-level borsh attribute — is accepted.
        assert!(state_accepts(StructOrEnumItem::Struct(parse_quote! {
            pub struct S {}
        })));
    }
}
