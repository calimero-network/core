//! `#[app::mergeable]` — opt a custom struct into app-defined merge dispatch.
//!
//! `#[derive(Mergeable)]` gives a struct a merge function; it does not give the
//! storage layer any reason to call it. A collection entry is merged by
//! matching on its `crdt_type`, and an entry whose value type declares nothing
//! resolves last-write-wins with the app's `merge` never consulted. Its CRDT
//! fields still converge — they are separate child entities under deterministic
//! ids — so the loss is of app-defined *semantics*, not of data.
//!
//! This attribute supplies the missing declaration: a `CustomTypeId` to stamp
//! on the entry and dispatch on.
//!
//! It is deliberately separate from the derive. Dispatch costs a wasm call per
//! entry conflict, so a struct that only needs field-by-field delegation — the
//! common case, and what the derive generates — should not pay it.

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::{Data, DeriveInput, LitStr};

use crate::errors::Errors;
use crate::forbidden_types::validate_fields_allowing_bare;
use crate::rekey::generate_struct_rekey;

/// Optional `id = "..."` override for the digest's input path.
pub struct Args {
    id: Option<LitStr>,
}

impl syn::parse::Parse for Args {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        if input.is_empty() {
            return Ok(Self { id: None });
        }
        let key: syn::Ident = input.parse()?;
        if key != "id" {
            return Err(syn::Error::new(
                key.span(),
                "(calimero)> expected `id = \"...\"`",
            ));
        }
        let _: syn::Token![=] = input.parse()?;
        Ok(Self {
            id: Some(input.parse()?),
        })
    }
}

pub fn expand(args: &Args, input: DeriveInput) -> TokenStream {
    let errors = Errors::default();
    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        Data::Struct(s) => {
            // Bare primitives are legal HERE and nowhere else: this is the one
            // construct where a plain field converges by a rule the app
            // declared, rather than silently drifting. The determinism rules
            // (interior mutability, iteration-order-dependent std collections)
            // still apply — no app-defined merge can repair those.
            validate_fields_allowing_bare(&s.fields, &errors);
            &s.fields
        }
        Data::Enum(_) | Data::Union(_) => {
            return quote_spanned! {ident.span()=>
                ::core::compile_error!(
                    "(calimero)> #[app::mergeable] applies to structs — an enum has no \
                     canonical merge rule across differing variants. Implement Mergeable by \
                     hand, or wrap the enum in `LwwRegister<T>` for last-write-wins."
                );
            };
        }
    };

    if let Err(errs) = errors.check() {
        return errs.to_compile_error();
    }

    // The digest's input. `module_path!()` resolves where the struct is
    // declared, so the default is the type's real path rather than a bare
    // ident — two `Stats` in different modules must not collide.
    //
    // The override exists because the default is only as stable as the path:
    // moving the type between modules, or renaming the crate, changes the id
    // and orphans every entry already stamped with the old one. An app that
    // expects to reorganise pins the id instead.
    let type_path: TokenStream = args.id.as_ref().map_or_else(
        || {
            let name = ident.to_string();
            quote! { ::core::concat!(::core::module_path!(), "::", #name) }
        },
        |id| quote! { #id },
    );

    let rekey_body = generate_struct_rekey(fields);
    let rekey_register_body = crate::state::rekey_register_calls(fields);

    quote! {
        #input

        impl #impl_generics ::calimero_storage::collections::crdt_meta::CustomMergeable
            for #ident #ty_generics #where_clause
        {
            const TYPE_ID:
                ::calimero_storage::collections::crdt_meta::CustomTypeId =
                ::calimero_storage::collections::crdt_meta::CustomTypeId::of(#type_path);

            // Generated here rather than defaulted on the trait: `Self` is
            // concrete at this point, so the `Mergeable` + borsh bounds the
            // registry needs are satisfiable without the trait carrying them.
            // A missing `impl Mergeable for` this type surfaces as an
            // unsatisfied bound on this call.
            fn register_merge() -> bool {
                ::calimero_storage::merge::register_custom_merge::<Self>()
            }
        }

        // Declaring the type is what makes dispatch reachable: the entry is
        // stamped from here, and `save_internal` matches on that stamp. Without
        // it the merge function is unreferenced by anything the storage layer
        // consults.
        impl #impl_generics ::calimero_storage::collections::CrdtMeta
            for #ident #ty_generics #where_clause
        {
            fn crdt_type() -> ::calimero_storage::collections::CrdtType {
                ::calimero_storage::collections::CrdtType::Custom(
                    <Self as ::calimero_storage::collections::crdt_meta::CustomMergeable>::TYPE_ID,
                )
            }

            // Structured, not Blob: the struct's collection fields live as
            // child entities and converge on their own. Only the remaining
            // plain fields travel in the value blob this merge is handed.
            fn storage_strategy()
                -> ::calimero_storage::collections::StorageStrategy
            {
                ::calimero_storage::collections::StorageStrategy::Structured
            }

            fn can_contain_crdts() -> bool {
                true
            }
        }

        // Declares HOW this type merges: dispatched. The entry is stamped with
        // `TYPE_ID` and the merge point calls the rule above, rather than
        // resolving the entry last-write-wins.
        impl #impl_generics ::calimero_storage::collections::MergeStrategy
            for #ident #ty_generics #where_clause
        {
            const DISPATCHED: bool = true;
        }

        // Same re-keying the derive generates. A hand-written `Mergeable` has
        // to supply this by hand today, and forgetting it silently loses the
        // nested collections' concurrent writes — so the attribute emits it
        // rather than leaving it as a documented footgun.
        impl #impl_generics ::calimero_storage::collections::rekey::RekeyTarget
            for #ident #ty_generics #where_clause
        {
            fn rekey_relative_to(
                &mut self,
                parent_id: ::calimero_storage::address::Id,
            ) {
                #rekey_body
            }

            fn register_nested_value_types() {
                #rekey_register_body
            }

            // Hooks this type's merge into the registration walk. The walk is
            // the re-key one; riding it means app-defined merge reaches the
            // whole value graph, cascade and termination guard included,
            // instead of one level down from the root.
            fn register_own_custom_merge() {
                let _ = <Self as
                    ::calimero_storage::collections::crdt_meta::CustomMergeable
                >::register_merge();
            }
        }
    }
}
