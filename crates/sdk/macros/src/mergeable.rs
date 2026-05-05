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
