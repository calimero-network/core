//! `#[app::state(version = N)]`: the state root holds only collection handles, so loading
//! it reads nothing. Each field is a table or collection named after the field in camelCase,
//! the name the TypeScript SDK gives it too.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Fields, ItemStruct, LitInt};

use crate::types::{camel, map_value, plain_abi_type};

pub fn expand(args: TokenStream, item: &ItemStruct) -> syn::Result<TokenStream> {
    let sdk = crate::sdk::path();
    let mut version: Option<u32> = None;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("version") {
            let lit: LitInt = meta.value()?.parse()?;
            version = Some(lit.base10_parse()?);
            Ok(())
        } else {
            Err(meta.error("expected `version = N`"))
        }
    });
    syn::parse::Parser::parse2(parser, args)?;
    let Some(version) = version else {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "#[app::state] needs `version = N`",
        ));
    };
    if version == 0 {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "state versions start at 1",
        ));
    }
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "the app state takes no generics",
        ));
    }
    let Fields::Named(fields) = &item.fields else {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "the app state is a struct with named fields",
        ));
    };
    let name = &item.ident;
    let inits = fields.named.iter().map(|f| {
        // SAFETY: guaranteed by `Fields::Named`, whose fields all have identifiers.
        let ident = f.ident.as_ref().expect("named field");
        let ty = &f.ty;
        let storage = camel(&ident.to_string());
        quote! { #ident: <#ty>::new(#storage) }
    });
    // `{"collections":{..},"fields":{"<name>":"<kind>",..},"search":{..},"version":N}`, keys sorted (see `types::canonical`);
    // each kind is the field type's `StateField::KIND`, joined at compile time.
    let mut kinds: Vec<(String, &syn::Type)> = fields
        .named
        .iter()
        .map(|f| {
            // SAFETY: guaranteed by `Fields::Named`, whose fields all have identifiers.
            let ident = f.ident.as_ref().expect("named field");
            (camel(&ident.to_string()), &f.ty)
        })
        .collect();
    kinds.sort_by(|a, b| a.0.cmp(&b.0));
    // The value type of every field written `UnorderedMap<K, V>`, by its storage name: the
    // map's schema below, and what the Rust host path reads 64-bit strings back by.
    let map_values: Vec<(String, String)> = kinds
        .iter()
        .filter_map(|(field, ty)| Some((field.clone(), plain_abi_type(map_value(ty)?))))
        .collect();
    // `"collections":{"<name>":<schema>,..}`: every field's `StateField::SCHEMA`, or
    // `{"kind":"<KIND>"}` when it has none, or for a map `{"kind":"unorderedMap",
    // "value":"<type>"}`; the host keeps its keys in step with these and converts records
    // and map values by their types.
    let mut parts = vec![quote! { "{\"collections\":{" }];
    for (i, (field, ty)) in kinds.iter().enumerate() {
        let open = format!("{}\"{field}\":", if i == 0 { "" } else { "," });
        parts.push(quote! { #open });
        if let Some((_, value)) = map_values.iter().find(|(name, _)| name == field) {
            let schema = format!(
                "{{\"kind\":\"unorderedMap\",\"value\":{}}}",
                serde_json::Value::String(value.clone())
            );
            parts.push(quote! { #schema });
            continue;
        }
        parts.push(quote! {
            if <#ty as #sdk::app::StateField>::SCHEMA.is_empty() { "{\"kind\":\"" }
            else { <#ty as #sdk::app::StateField>::SCHEMA }
        });
        parts.push(quote! {
            if <#ty as #sdk::app::StateField>::SCHEMA.is_empty() { <#ty as #sdk::app::StateField>::KIND }
            else { "" }
        });
        parts.push(quote! {
            if <#ty as #sdk::app::StateField>::SCHEMA.is_empty() { "\"}" } else { "" }
        });
    }
    parts.push(quote! { "},\"fields\":{" });
    for (i, (field, ty)) in kinds.iter().enumerate() {
        let open = format!("{}\"{field}\":\"", if i == 0 { "" } else { "," });
        parts.push(quote! { #open });
        parts.push(quote! { <#ty as #sdk::app::StateField>::KIND });
        parts.push(quote! { "\"" });
    }
    // `"search":{"<name>":[field,..],..}`: every field's `StateField::SEARCH` (an empty list
    // for anything but a searchable table; the host skips those).
    parts.push(quote! { "},\"search\":{" });
    for (i, (field, ty)) in kinds.iter().enumerate() {
        let open = format!("{}\"{field}\":", if i == 0 { "" } else { "," });
        parts.push(quote! { #open });
        parts.push(quote! { <#ty as #sdk::app::StateField>::SEARCH });
    }
    let close = format!("}},\"version\":{version}}}");
    let map_names = map_values.iter().map(|(name, _)| name);
    let map_types = map_values.iter().map(|(_, ty)| ty);
    parts.push(quote! { #close });
    Ok(quote! {
        #item

        impl #sdk::app::AppState for #name {
            const VERSION: u32 = #version;
            const ABI: &'static str = {
                const __PARTS: &[&str] = &[#(#parts),*];
                const __LEN: usize = #sdk::__private::abi_len(__PARTS);
                const __BYTES: [u8; __LEN] =
                    #sdk::__private::abi_concat::<__LEN>(__PARTS);
                match ::core::str::from_utf8(&__BYTES) {
                    ::core::result::Result::Ok(abi) => abi,
                    ::core::result::Result::Err(_) => panic!("the state ABI is ASCII"),
                }
            };

            fn load() -> Self {
                #sdk::__private::map_value_types(&[#((#map_names, #map_types)),*]);
                Self { #(#inits),* }
            }
        }
    })
}
