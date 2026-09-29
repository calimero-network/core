//! `#[derive(Searchable)]` — declare which fields of a collection's value the
//! node's full-text index holds, and how.
//!
//! ```ignore
//! #[derive(BorshSerialize, BorshDeserialize, Searchable)]
//! pub struct Message {
//!     #[search(keyword)] pub sender: LwwRegister<String>,
//!     #[search(text, infix)] pub text: LwwRegister<String>,
//!     #[search(text, weight = 200, name = "subject")] pub title: LwwRegister<String>,
//!     #[search(number)] pub ts: LwwRegister<u64>,
//!     pub internal: LwwRegister<String>,
//! }
//! ```
//!
//! * `text`: tokenized free text, BM25-ranked. `weight = N` boosts it (in
//!   hundredths, default 100); `infix` also indexes trigrams, so the field
//!   answers substring queries. The field's type must be `SearchText`.
//! * `keyword`: an exact value to filter on, never tokenized or scored
//!   (`SearchText`).
//! * `number`: a `u64` to range-filter on (`SearchNumber`).
//! * `name = "..."` renames the field in the index; the default is the Rust
//!   field name.
//!
//! A field without `#[search]` is not indexed. Fields appear in the schema in
//! declaration order; renaming, retyping or reordering them changes the schema
//! and makes the node rebuild the index.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Fields, Ident, LitInt, LitStr};

/// How one field is indexed.
enum Kind {
    Text { weight: u32, infix: bool },
    Keyword,
    Number,
}

struct FieldDecl {
    ident: Ident,
    name: LitStr,
    kind: Kind,
}

pub fn derive(input: DeriveInput) -> TokenStream {
    match expand(&input) {
        Ok(tokens) => tokens,
        Err(error) => error.to_compile_error(),
    }
}

fn parse_field(ident: &Ident, attr: &syn::Attribute) -> syn::Result<FieldDecl> {
    let mut kind: Option<Kind> = None;
    let mut weight: Option<LitInt> = None;
    let mut infix = false;
    let mut name = LitStr::new(&ident.to_string(), ident.span());
    attr.parse_nested_meta(|meta| {
        let set_kind = |kind: &mut Option<Kind>, new: Kind| {
            if kind.is_some() {
                return Err(meta.error(
                    "(calimero)> a #[search] field takes exactly one of `text`, `keyword`, `number`",
                ));
            }
            *kind = Some(new);
            Ok(())
        };
        if meta.path.is_ident("text") {
            set_kind(
                &mut kind,
                Kind::Text {
                    weight: 100,
                    infix: false,
                },
            )
        } else if meta.path.is_ident("keyword") {
            set_kind(&mut kind, Kind::Keyword)
        } else if meta.path.is_ident("number") {
            set_kind(&mut kind, Kind::Number)
        } else if meta.path.is_ident("weight") {
            weight = Some(meta.value()?.parse()?);
            Ok(())
        } else if meta.path.is_ident("infix") {
            infix = true;
            Ok(())
        } else if meta.path.is_ident("name") {
            name = meta.value()?.parse()?;
            Ok(())
        } else {
            Err(meta.error(
                "(calimero)> unknown #[search] option (expected `text`, `keyword`, `number`, \
                 `weight = N`, `infix` or `name = \"...\"`)",
            ))
        }
    })?;
    let kind = match (kind, weight, infix) {
        (None, ..) => {
            return Err(syn::Error::new(
                attr.span(),
                "(calimero)> #[search] needs one of `text`, `keyword`, `number`",
            ))
        }
        (Some(Kind::Text { .. }), weight, infix) => Kind::Text {
            weight: weight.map_or(Ok(100), |w| w.base10_parse())?,
            infix,
        },
        (Some(_), Some(weight), _) => {
            return Err(syn::Error::new(
                weight.span(),
                "(calimero)> `weight` applies to a `text` field only",
            ))
        }
        (Some(_), None, true) => {
            return Err(syn::Error::new(
                attr.span(),
                "(calimero)> `infix` applies to a `text` field only",
            ))
        }
        (Some(kind), None, false) => kind,
    };
    Ok(FieldDecl {
        ident: ident.clone(),
        name,
        kind,
    })
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new(
            input.ident.span(),
            "(calimero)> #[derive(Searchable)] supports structs with named fields only",
        ));
    };
    let Fields::Named(named) = &data.fields else {
        return Err(syn::Error::new(
            data.fields.span(),
            "(calimero)> #[derive(Searchable)] supports structs with named fields only",
        ));
    };

    let mut decls: Vec<FieldDecl> = Vec::new();
    for field in &named.named {
        let Some(ident) = &field.ident else { continue };
        let mut attrs = field.attrs.iter().filter(|a| a.path().is_ident("search"));
        let Some(attr) = attrs.next() else { continue };
        if let Some(again) = attrs.next() {
            return Err(syn::Error::new(
                again.span(),
                "(calimero)> a field takes one #[search] attribute",
            ));
        }
        let decl = parse_field(ident, attr)?;
        if decls.iter().any(|d| d.name.value() == decl.name.value()) {
            return Err(syn::Error::new(
                decl.name.span(),
                format!("(calimero)> two fields are indexed as `{}`", decl.name.value()),
            ));
        }
        if decl.name.value().starts_with('_') || decl.name.value().contains('.') {
            return Err(syn::Error::new(
                decl.name.span(),
                "(calimero)> an indexed field's name may not start with `_` or contain `.` \
                 (the index reserves them)",
            ));
        }
        decls.push(decl);
    }
    if decls.is_empty() {
        return Err(syn::Error::new(
            input.ident.span(),
            "(calimero)> #[derive(Searchable)] needs at least one #[search(...)] field",
        ));
    }

    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let schema = decls.iter().map(|decl| {
        let name = &decl.name;
        let kind = match decl.kind {
            Kind::Text { weight, infix } => quote! {
                ::calimero_sdk::search::SearchFieldKind::Text { weight: #weight, infix: #infix }
            },
            Kind::Keyword => quote! { ::calimero_sdk::search::SearchFieldKind::Keyword },
            Kind::Number => quote! { ::calimero_sdk::search::SearchFieldKind::U64 },
        };
        quote! {
            ::calimero_sdk::search::SearchFieldSchema {
                name: ::std::string::String::from(#name),
                kind: #kind,
            }
        }
    });
    let values = decls.iter().map(|decl| {
        let (name, field) = (&decl.name, &decl.ident);
        let value = match decl.kind {
            Kind::Text { .. } | Kind::Keyword => quote! {
                ::calimero_sdk::search::SearchText::search_text(&self.#field)
                    .map(::calimero_sdk::search::SearchValue::Str)
            },
            Kind::Number => quote! {
                ::calimero_sdk::search::SearchNumber::search_number(&self.#field)
                    .map(::calimero_sdk::search::SearchValue::U64)
            },
        };
        quote! {
            if let ::core::option::Option::Some(value) = #value {
                out.push((::std::string::String::from(#name), value));
            }
        }
    });
    let count = decls.len();

    Ok(quote! {
        impl #impl_generics ::calimero_sdk::search::Searchable for #ident #ty_generics #where_clause {
            fn search_fields() -> ::std::vec::Vec<::calimero_sdk::search::SearchFieldSchema> {
                ::std::vec![ #( #schema ),* ]
            }

            fn search_document(
                &self,
            ) -> ::std::vec::Vec<(::std::string::String, ::calimero_sdk::search::SearchValue)> {
                let mut out = ::std::vec::Vec::with_capacity(#count);
                #( #values )*
                out
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::*;

    fn expand_str(ts: TokenStream) -> String {
        derive(syn::parse2(ts).expect("parse DeriveInput")).to_string()
    }

    #[test]
    fn fields_declare_their_kind_in_order_and_unmarked_ones_stay_out() {
        let out = expand_str(quote! {
            struct Message {
                #[search(keyword)] sender: LwwRegister<String>,
                #[search(text, infix, weight = 150)] text: LwwRegister<String>,
                #[search(number, name = "at")] ts: LwwRegister<u64>,
                internal: LwwRegister<String>,
            }
        });
        let sender = out.find("\"sender\"").expect(&out);
        let text = out.find("\"text\"").expect(&out);
        let at = out.find("\"at\"").expect(&out);
        assert!(sender < text && text < at, "declaration order: {out}");
        assert!(out.contains("SearchFieldKind :: Keyword"), "{out}");
        assert!(
            out.contains("SearchFieldKind :: Text { weight : 150u32 , infix : true }"),
            "{out}"
        );
        assert!(out.contains("SearchNumber :: search_number (& self . ts)"), "{out}");
        assert!(!out.contains("internal"), "an unmarked field is not indexed: {out}");
    }

    #[test]
    fn a_text_field_defaults_to_weight_100_without_infix() {
        let out = expand_str(quote! {
            struct Note { #[search(text)] body: String }
        });
        assert!(
            out.contains("SearchFieldKind :: Text { weight : 100u32 , infix : false }"),
            "{out}"
        );
    }

    #[test]
    fn misdeclarations_are_compile_errors() {
        for (case, input) in [
            ("no field", quote! { struct A { x: String } }),
            ("no kind", quote! { struct A { #[search(name = "y")] x: String } }),
            ("two kinds", quote! { struct A { #[search(text, keyword)] x: String } }),
            ("unknown option", quote! { struct A { #[search(unique)] x: String } }),
            ("weight off text", quote! { struct A { #[search(keyword, weight = 2)] x: String } }),
            ("infix off text", quote! { struct A { #[search(number, infix)] x: u64 } }),
            (
                "duplicate name",
                quote! { struct A { #[search(text)] x: String, #[search(keyword, name = "x")] y: String } },
            ),
            ("reserved name", quote! { struct A { #[search(text, name = "_id")] x: String } }),
            ("dotted name", quote! { struct A { #[search(text, name = "a.b")] x: String } }),
            (
                "two attributes",
                quote! { struct A { #[search(text)] #[search(keyword)] x: String } },
            ),
            ("tuple struct", quote! { struct A(#[search(text)] String); }),
            ("enum", quote! { enum A { X } }),
        ] {
            let out = expand_str(input);
            assert!(out.contains("compile_error"), "{case}: {out}");
            assert!(out.contains("(calimero)>"), "{case}: {out}");
        }
    }
}
