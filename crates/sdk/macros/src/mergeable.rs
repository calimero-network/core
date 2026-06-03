//! `#[derive(Mergeable)]` — opt-in derive for user-defined structs that need to
//! participate in CRDT merging.
//!
//! Why a derive at all? `UnorderedMap<K, V>`, `Vector<V>` etc. require
//! `V: Mergeable`. Without this derive, users would have to hand-roll the impl
//! for every struct they want to put inside a Calimero collection — easy to get
//! wrong, easy to silently merge incorrectly. The derive applies the same
//! forbidden-type lint as `#[app::state]` recursively to each field, then
//! generates a field-by-field `merge()` that delegates to each field's own
//! `Mergeable` impl.

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::{Data, DeriveInput, Fields};

use crate::errors::Errors;
use crate::forbidden_types::validate_fields;

pub fn derive(input: DeriveInput) -> TokenStream {
    let errors = Errors::default();

    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let merge_body: TokenStream = match &input.data {
        Data::Struct(s) => {
            validate_fields(&s.fields, &errors);
            generate_struct_merge(&s.fields)
        }
        Data::Enum(_) => {
            // Enum merge is ambiguous (different variants on each side — which
            // wins?). Reject; users that need it can `impl Mergeable` by hand
            // or wrap the enum in `LwwRegister<MyEnum>` for LWW semantics.
            return quote_spanned! {input.ident.span()=>
                ::core::compile_error!(
                    "(calimero)> #[derive(Mergeable)] on enums is not supported — \
                     enum variants have no canonical merge rule. Implement Mergeable manually \
                     or wrap the enum in `LwwRegister<MyEnum>` for last-write-wins semantics."
                );
            };
        }
        Data::Union(_) => {
            return quote_spanned! {input.ident.span()=>
                ::core::compile_error!(
                    "(calimero)> #[derive(Mergeable)] is not supported on unions"
                );
            };
        }
    };

    if let Err(errs) = errors.check() {
        return errs.to_compile_error();
    }

    let rekey_body: TokenStream = match &input.data {
        Data::Struct(s) => generate_struct_rekey(&s.fields),
        _ => quote! {},
    };

    // Registration of THIS struct's nested value types, scanned exactly like the
    // root `#[app::state]` does its own fields. The root only ever names the type
    // tokens in its OWN fields, so a custom struct reachable only through another
    // custom struct's collection is never registered there; emitting the same
    // scan per Mergeable struct cascades registration through the whole value
    // graph (see `RekeyTarget::register_nested_value_types`).
    let rekey_register_body: TokenStream = match &input.data {
        Data::Struct(s) => crate::state::rekey_register_calls(&s.fields),
        _ => quote! {},
    };

    quote! {
        impl #impl_generics ::calimero_storage::collections::Mergeable
            for #ident #ty_generics #where_clause
        {
            fn merge(
                &mut self,
                other: &Self,
            ) -> ::core::result::Result<
                (),
                ::calimero_storage::collections::crdt_meta::MergeError,
            > {
                #merge_body
                ::core::result::Result::Ok(())
            }
        }

        // Deterministic re-keying for use as a CRDT collection VALUE. When this
        // struct is stored as a map/set/vector value under a deterministic entry
        // id, each field's nested collection ids are re-keyed under a
        // field-namespaced child of that id — so every replica derives identical
        // ids and the nested CRDTs converge as child entities instead of the
        // whole struct blob being last-writer-wins'd (the #2577 data loss).
        // `rekey_field_if_supported!` autoref-dispatches: a real re-key for
        // fields whose type implements `RekeyTarget` (collections, nested CRDT
        // structs), a no-op for leaf fields (e.g. `LwwRegister`).
        impl #impl_generics ::calimero_storage::collections::rekey::RekeyTarget
            for #ident #ty_generics #where_clause
        {
            fn rekey_relative_to(
                &mut self,
                parent_id: ::calimero_storage::address::Id,
            ) {
                #rekey_body
            }

            // Cascade registration into the value types this struct nests, so a
            // custom struct reachable only through this one's collections is
            // registered too (not left to be last-writer-wins'd). Same per-field
            // scan the root state runs; `register_rekey_if_supported!` recurses
            // and the first-registration guard makes self-referential value
            // graphs terminate.
            fn register_nested_value_types() {
                #rekey_register_body
            }
        }
    }
}

fn generate_struct_rekey(fields: &Fields) -> TokenStream {
    match fields {
        Fields::Named(named) => {
            let calls = named.named.iter().map(|f| {
                let name = f.ident.as_ref().expect("named field has ident");
                let name_str = name.to_string();
                quote! {
                    ::calimero_storage::rekey_field_if_supported!(
                        &mut self.#name,
                        ::calimero_storage::collections::rekey::field_child_id(parent_id, #name_str)
                    );
                }
            });
            quote! { #(#calls)* }
        }
        Fields::Unnamed(unnamed) => {
            let calls = unnamed.unnamed.iter().enumerate().map(|(i, _)| {
                let idx = syn::Index::from(i);
                let name_str = i.to_string();
                quote! {
                    ::calimero_storage::rekey_field_if_supported!(
                        &mut self.#idx,
                        ::calimero_storage::collections::rekey::field_child_id(parent_id, #name_str)
                    );
                }
            });
            quote! { #(#calls)* }
        }
        Fields::Unit => quote! {},
    }
}

fn generate_struct_merge(fields: &Fields) -> TokenStream {
    match fields {
        Fields::Named(named) => {
            let calls = named.named.iter().map(|f| {
                let name = f.ident.as_ref().expect("named field has ident");
                quote! {
                    ::calimero_storage::collections::Mergeable::merge(
                        &mut self.#name,
                        &other.#name,
                    )?;
                }
            });
            quote! { #(#calls)* }
        }
        Fields::Unnamed(unnamed) => {
            let calls = unnamed.unnamed.iter().enumerate().map(|(i, _)| {
                let idx = syn::Index::from(i);
                quote! {
                    ::calimero_storage::collections::Mergeable::merge(
                        &mut self.#idx,
                        &other.#idx,
                    )?;
                }
            });
            quote! { #(#calls)* }
        }
        Fields::Unit => quote! {},
    }
}
