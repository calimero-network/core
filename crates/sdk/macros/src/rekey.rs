//! The per-field re-key cascade, shared by the two derives that need it.
//!
//! `#[app::state]` and `#[derive(Mergeable)]` both emit a `rekey_relative_to`
//! body, and both emitted it from their own copy of this function — identical
//! apart from whether `Fields` was imported or written `syn::Fields`. That is a
//! bad place for two copies to live: re-keying is where #2577 (*custom struct
//! CRDT values lost data*) and #2581 (*nested-through-custom-struct silently
//! LWW'd*) both were, and both were silent data loss rather than a failure. A
//! divergence between a root struct's cascade and a nested one's is exactly that
//! shape of bug again, so the cascade is defined once here.

use proc_macro2::TokenStream;
use quote::quote;
use syn::Fields;

/// Emit the per-field re-key cascade for a struct's `rekey_relative_to`.
///
/// Each field is re-keyed under a field-namespaced child id.
/// `rekey_field_if_supported!` autoref-dispatches — a real re-key for
/// `RekeyTarget` fields, a no-op for leaves — and each expansion stays in its own
/// block, because that macro defines per-invocation helper traits.
///
/// Tuple-struct fields are namespaced by their positional index rendered as a
/// string, so a named field `0` and a tuple field `.0` land on the same child id.
/// That is deliberate: the id has to be stable across a struct gaining names, not
/// unique across two shapes the same struct cannot have at once.
pub(crate) fn generate_struct_rekey(fields: &Fields) -> TokenStream {
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
