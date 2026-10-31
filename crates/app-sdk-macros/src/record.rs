//! `#[derive(Record)]` with `#[table(key = .., unique(..), index(..), count(..), search = [..])]`.
//!
//! The derive implements `calimero_app_sdk::Record` and serde for the struct: field names
//! are camelCase in both CBOR (stored values) and JSON (results); `u64`/`i64` fields are
//! integers in CBOR and decimal strings in JSON (D20, Ruling 25). `partition` is reserved
//! and refused (D48).

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::meta::ParseNestedMeta;
use syn::punctuated::Punctuated;
use syn::{DeriveInput, Fields, Ident, LitStr, Token};

use crate::types::{camel, check_wire, wire, Wire};

/// A named set of fields, as written: `by_owner = ["owner", "name_key"]`.
struct FieldSet {
    name: String,
    fields: Vec<LitStr>,
}

#[derive(Default)]
struct TableAttr {
    key: Vec<LitStr>,
    unique: Vec<FieldSet>,
    index: Vec<FieldSet>,
    count: Vec<FieldSet>,
    /// Text fields the host indexes for full-text search (spec section 7).
    search: Vec<LitStr>,
}

/// `"a"` or `["a", "b"]`.
fn field_list(meta: &ParseNestedMeta<'_>) -> syn::Result<Vec<LitStr>> {
    let value = meta.value()?;
    if value.peek(syn::token::Bracket) {
        let content;
        let _ = syn::bracketed!(content in value);
        let list = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
        return Ok(list.into_iter().collect());
    }
    Ok(vec![value.parse()?])
}

fn field_sets(meta: &ParseNestedMeta<'_>, into: &mut Vec<FieldSet>) -> syn::Result<()> {
    meta.parse_nested_meta(|inner| {
        let Some(ident) = inner.path.get_ident() else {
            return Err(inner.error("expected `name = [\"field\", ..]`"));
        };
        let fields = field_list(&inner)?;
        if fields.is_empty() {
            return Err(inner.error("a field set names at least one field"));
        }
        into.push(FieldSet {
            name: camel(&ident.to_string()),
            fields,
        });
        Ok(())
    })
}

fn parse_table(input: &DeriveInput) -> syn::Result<TableAttr> {
    let mut table = TableAttr::default();
    let mut seen = false;
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("table")) {
        seen = true;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("key") {
                table.key = field_list(&meta)?;
                Ok(())
            } else if meta.path.is_ident("unique") {
                field_sets(&meta, &mut table.unique)
            } else if meta.path.is_ident("index") {
                field_sets(&meta, &mut table.index)
            } else if meta.path.is_ident("count") {
                field_sets(&meta, &mut table.count)
            } else if meta.path.is_ident("search") {
                table.search = field_list(&meta)?;
                Ok(())
            } else if meta.path.is_ident("partition") {
                Err(meta.error("partition is reserved (D48)"))
            } else {
                Err(meta.error("expected `key`, `unique`, `index`, `count` or `search`"))
            }
        })?;
    }
    if !seen || table.key.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "a record needs `#[table(key = \"field\")]`",
        ));
    }
    Ok(table)
}

pub fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let table = parse_table(input)?;
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "records take no generics",
        ));
    }
    let syn::Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "a record is a struct",
        ));
    };
    let Fields::Named(named) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "a record has named fields",
        ));
    };
    let fields: Vec<(Ident, &syn::Type)> = named
        .named
        .iter()
        // SAFETY: guaranteed by `Fields::Named`, whose fields all have identifiers.
        .map(|f| (f.ident.clone().expect("named field"), &f.ty))
        .collect();

    for (_, ty) in &fields {
        check_wire(ty)?;
    }

    // Every listed field must exist; lists use Rust names, the trait uses camelCase.
    let resolve = |lit: &LitStr| -> syn::Result<String> {
        let wanted = lit.value();
        if fields.iter().any(|(ident, _)| *ident == wanted) {
            Ok(camel(&wanted))
        } else {
            Err(syn::Error::new(lit.span(), format!("no field `{wanted}`")))
        }
    };
    let key = table
        .key
        .iter()
        .map(resolve)
        .collect::<syn::Result<Vec<_>>>()?;
    let sets = |sets: &[FieldSet]| -> syn::Result<TokenStream> {
        let mut out = Vec::new();
        for set in sets {
            let name = &set.name;
            let list = set
                .fields
                .iter()
                .map(resolve)
                .collect::<syn::Result<Vec<_>>>()?;
            out.push(quote! { (#name, &[#(#list),*]) });
        }
        Ok(quote! { &[#(#out),*] })
    };
    let search = table
        .search
        .iter()
        .map(resolve)
        .collect::<syn::Result<Vec<_>>>()?;
    // The same list as JSON, for the state ABI's `search` (joined at compile time).
    let search_json = format!(
        "[{}]",
        search
            .iter()
            .map(|f| format!("\"{f}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    let unique = sets(&table.unique)?;
    let index = sets(&table.index)?;
    let count = sets(&table.count)?;

    let name = &input.ident;
    let arms = fields.iter().map(|(ident, _)| {
        let key = camel(&ident.to_string());
        quote! { #key => ::calimero_app_sdk::ToKeyPart::to_key_part(&self.#ident) }
    });

    let ser_fields = fields.iter().map(|(ident, ty)| {
        let rename = camel(&ident.to_string());
        match wire(ty) {
            // Bytes are borrowed (the helpers are generic over `AsRef<[u8]>`).
            Some(w @ Wire::Bytes) => {
                let with = w.auto_module();
                quote! { #[serde(rename = #rename, with = #with)] #ident: &'__rec #ty }
            }
            Some(w @ Wire::OptBytes) => {
                let with = w.auto_module();
                quote! {
                    #[serde(rename = #rename, with = #with)]
                    #ident: ::core::option::Option<&'__rec [u8]>
                }
            }
            // 64-bit integers are copied.
            Some(w) => {
                let with = w.auto_module();
                quote! { #[serde(rename = #rename, with = #with)] #ident: #ty }
            }
            None => quote! { #[serde(rename = #rename)] #ident: &'__rec #ty },
        }
    });
    let ser_values = fields.iter().map(|(ident, ty)| match wire(ty) {
        Some(Wire::Bytes) | None => quote! { #ident: &self.#ident },
        Some(Wire::OptBytes) => quote! { #ident: self.#ident.as_deref() },
        Some(_) => quote! { #ident: self.#ident },
    });
    let de_fields = fields.iter().map(|(ident, ty)| {
        let rename = camel(&ident.to_string());
        match wire(ty) {
            Some(int) if int.is_option() => {
                let with = int.auto_module();
                quote! { #[serde(rename = #rename, default, with = #with)] #ident: #ty }
            }
            Some(int) => {
                let with = int.auto_module();
                quote! { #[serde(rename = #rename, with = #with)] #ident: #ty }
            }
            None => quote! { #[serde(rename = #rename)] #ident: #ty },
        }
    });
    let idents: Vec<&Ident> = fields.iter().map(|(ident, _)| ident).collect();
    let ser = format_ident!("__{}Ser", name, span = Span::call_site());
    let de = format_ident!("__{}De", name, span = Span::call_site());

    Ok(quote! {
        impl ::calimero_app_sdk::Record for #name {
            const KEY: &'static [&'static str] = &[#(#key),*];
            const UNIQUE: &'static [::calimero_app_sdk::table::FieldSet] = #unique;
            const INDEXES: &'static [::calimero_app_sdk::table::FieldSet] = #index;
            const COUNTS: &'static [::calimero_app_sdk::table::FieldSet] = #count;
            const SEARCH: &'static [&'static str] = &[#(#search),*];
            const SEARCH_JSON: &'static str = #search_json;

            fn key_parts(&self, fields: &[&str]) -> ::std::vec::Vec<::calimero_app_sdk::KeyPart> {
                fields
                    .iter()
                    .map(|f| match *f {
                        #(#arms,)*
                        // SAFETY: guaranteed by the derive: tables pass only the field lists
                        // above, each checked against the struct at compile time.
                        other => ::core::panic!("record has no field `{}`", other),
                    })
                    .collect()
            }
        }

        const _: () = {
            use ::calimero_app_sdk::__private::serde;

            #[derive(serde::Serialize)]
            #[serde(crate = "::calimero_app_sdk::__private::serde")]
            struct #ser<'__rec> {
                #(#ser_fields,)*
                #[serde(skip)]
                __marker: ::core::marker::PhantomData<&'__rec ()>,
            }

            #[derive(serde::Deserialize)]
            #[serde(crate = "::calimero_app_sdk::__private::serde")]
            struct #de {
                #(#de_fields,)*
            }

            impl serde::Serialize for #name {
                fn serialize<S: serde::Serializer>(&self, s: S) -> ::core::result::Result<S::Ok, S::Error> {
                    #ser { #(#ser_values,)* __marker: ::core::marker::PhantomData }.serialize(s)
                }
            }

            impl<'de> serde::Deserialize<'de> for #name {
                fn deserialize<D: serde::Deserializer<'de>>(d: D) -> ::core::result::Result<Self, D::Error> {
                    let #de { #(#idents),* } = #de::deserialize(d)?;
                    ::core::result::Result::Ok(Self { #(#idents),* })
                }
            }
        };
    })
}
