//! `#[derive(Indexed)]` — declare an `IndexedMap` value's secondary indexes
//! with attributes instead of a hand-written `Indexed` impl.
//!
//! ```ignore
//! #[derive(BorshSerialize, BorshDeserialize, Indexed)]
//! #[index(name = "status_created", fields(status, created_at))]
//! pub struct Issue {
//!     #[index] pub status: LwwRegister<String>,
//!     #[index(name = "owner")] pub assignee: LwwRegister<Option<String>>,
//!     pub created_at: LwwRegister<u64>,
//! }
//! ```
//!
//! * `#[index]` on a field declares an index over that field, named after it;
//!   `#[index(name = "...")]` names it explicitly.
//! * `#[index(name = "...", fields(a, b, ...))]` on the struct declares a
//!   compound index over up to three fields, in that order. Its leading fields
//!   can be matched on their own, and the rest order the result.
//!
//! Field indexes come first in `INDEXES`, in declaration order, then compound
//! ones in attribute order. The order does not matter to storage — an index's
//! rows are keyed by its NAME — so reordering is free, while renaming an index
//! rebuilds it under the new name.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Fields, Ident, LitStr};

/// Compound keys are encoded as tuples, and `IndexValue` is implemented for
/// tuples of up to this many components.
const MAX_COMPOUND_FIELDS: usize = 3;

/// One declared index: its name and the fields its key is built from.
struct IndexDecl {
    name: LitStr,
    fields: Vec<Ident>,
}

pub fn derive(input: DeriveInput) -> TokenStream {
    match expand(&input) {
        Ok(tokens) => tokens,
        Err(error) => error.to_compile_error(),
    }
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new(
            input.ident.span(),
            "(calimero)> #[derive(Indexed)] supports structs with named fields only",
        ));
    };
    let Fields::Named(named) = &data.fields else {
        return Err(syn::Error::new(
            data.fields.span(),
            "(calimero)> #[derive(Indexed)] supports structs with named fields only",
        ));
    };
    let field_names: Vec<&Ident> = named
        .named
        .iter()
        .filter_map(|f| f.ident.as_ref())
        .collect();

    let mut decls = Vec::new();
    for field in &named.named {
        let Some(ident) = &field.ident else { continue };
        for attr in field.attrs.iter().filter(|a| a.path().is_ident("index")) {
            let mut name = LitStr::new(&ident.to_string(), ident.span());
            if !matches!(attr.meta, syn::Meta::Path(_)) {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("name") {
                        name = meta.value()?.parse()?;
                        Ok(())
                    } else {
                        Err(meta.error(
                            "(calimero)> unknown `#[index(...)]` option on a field (expected `name`)",
                        ))
                    }
                })?;
            }
            decls.push(IndexDecl {
                name,
                fields: vec![ident.clone()],
            });
        }
    }

    for attr in input.attrs.iter().filter(|a| a.path().is_ident("index")) {
        let mut name: Option<LitStr> = None;
        let mut fields: Vec<Ident> = Vec::new();
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                name = Some(meta.value()?.parse()?);
                Ok(())
            } else if meta.path.is_ident("fields") {
                meta.parse_nested_meta(|field| {
                    fields.push(field.path.require_ident()?.clone());
                    Ok(())
                })
            } else {
                Err(meta.error(
                    "(calimero)> unknown `#[index(...)]` option on the struct (expected `name` \
                     and `fields`)",
                ))
            }
        })?;
        let name = name.ok_or_else(|| {
            syn::Error::new(
                attr.span(),
                "(calimero)> a struct-level #[index] needs `name = \"...\"`",
            )
        })?;
        if fields.is_empty() || fields.len() > MAX_COMPOUND_FIELDS {
            return Err(syn::Error::new(
                attr.span(),
                format!(
                    "(calimero)> a struct-level #[index] takes 1 to {MAX_COMPOUND_FIELDS} fields, \
                     got {}",
                    fields.len()
                ),
            ));
        }
        if let Some(unknown) = fields.iter().find(|f| !field_names.contains(f)) {
            return Err(syn::Error::new(
                unknown.span(),
                format!(
                    "(calimero)> #[index] names `{unknown}`, which is not a field of this struct"
                ),
            ));
        }
        decls.push(IndexDecl { name, fields });
    }

    if decls.is_empty() {
        return Err(syn::Error::new(
            input.ident.span(),
            "(calimero)> #[derive(Indexed)] needs at least one #[index] — on a field, or \
             `#[index(name = \"...\", fields(...))]` on the struct",
        ));
    }
    for (i, decl) in decls.iter().enumerate() {
        if decls[..i]
            .iter()
            .any(|earlier| earlier.name.value() == decl.name.value())
        {
            return Err(syn::Error::new(
                decl.name.span(),
                format!("(calimero)> two indexes are named `{}`", decl.name.value()),
            ));
        }
    }

    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let names = decls.iter().map(|decl| &decl.name);
    let arms = decls.iter().enumerate().map(|(position, decl)| {
        let fields = &decl.fields;
        let key = match fields.as_slice() {
            [single] => quote! { &self.#single },
            many => quote! { &( #( &self.#many ),* ) },
        };
        quote! {
            #position => ::calimero_storage::collections::IndexValue::encode_index(#key, out),
        }
    });

    Ok(quote! {
        impl #impl_generics ::calimero_storage::collections::Indexed
            for #ident #ty_generics #where_clause
        {
            const INDEXES: &'static [&'static str] = &[ #( #names ),* ];

            fn index_keys(
                &self,
                index: usize,
                out: &mut ::std::vec::Vec<::std::vec::Vec<u8>>,
            ) {
                match index {
                    #( #arms )*
                    _ => {}
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn expand_str(ts: TokenStream) -> String {
        derive(syn::parse2(ts).expect("parse DeriveInput")).to_string()
    }

    #[test]
    fn field_indexes_are_named_after_their_fields_unless_renamed() {
        let out = expand_str(quote! {
            struct Issue {
                #[index] status: LwwRegister<String>,
                #[index(name = "owner")] assignee: LwwRegister<Option<String>>,
                title: LwwRegister<String>,
            }
        });
        assert!(
            out.contains("const INDEXES : & 'static [& 'static str] = & [\"status\" , \"owner\"]"),
            "{out}"
        );
        assert!(out.contains("0usize => :: calimero_storage :: collections :: IndexValue :: encode_index (& self . status , out)"), "{out}");
        assert!(out.contains("1usize => :: calimero_storage :: collections :: IndexValue :: encode_index (& self . assignee , out)"), "{out}");
        assert!(
            !out.contains("title"),
            "an unannotated field is not indexed: {out}"
        );
    }

    #[test]
    fn a_compound_index_is_a_tuple_of_its_fields_after_the_field_indexes() {
        let out = expand_str(quote! {
            #[index(name = "status_created", fields(status, created_at))]
            struct Issue {
                #[index] status: LwwRegister<String>,
                created_at: LwwRegister<u64>,
            }
        });
        assert!(out.contains("& [\"status\" , \"status_created\"]"), "{out}");
        assert!(
            out.contains("1usize => :: calimero_storage :: collections :: IndexValue :: encode_index (& (& self . status , & self . created_at) , out)"),
            "{out}"
        );
    }

    #[test]
    fn misdeclarations_are_compile_errors() {
        for (case, input) in [
            ("no index", quote! { struct A { x: u64 } }),
            (
                "duplicate name",
                quote! { struct A { #[index] x: u64, #[index(name = "x")] y: u64 } },
            ),
            (
                "unknown field",
                quote! { #[index(name = "c", fields(x, nope))] struct A { x: u64 } },
            ),
            (
                "no name",
                quote! { #[index(fields(x))] struct A { x: u64 } },
            ),
            (
                "too many fields",
                quote! { #[index(name = "c", fields(a, b, c, d))] struct A { a: u8, b: u8, c: u8, d: u8 } },
            ),
            ("tuple struct", quote! { struct A(#[index] u64); }),
            ("enum", quote! { enum A { X } }),
            (
                "unknown option",
                quote! { struct A { #[index(unique)] x: u64 } },
            ),
        ] {
            let out = expand_str(input);
            assert!(out.contains("compile_error"), "{case}: {out}");
            assert!(out.contains("(calimero)>"), "{case}: {out}");
        }
    }
}
