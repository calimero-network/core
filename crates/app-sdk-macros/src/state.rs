//! `#[app::state(version = N)]`: the state root holds only collection handles, so loading
//! it reads nothing. Each field is a table or collection named after the field in camelCase,
//! the name the TypeScript SDK gives it too.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Fields, ItemStruct, LitInt};

use crate::types::camel;

pub fn expand(args: TokenStream, item: &ItemStruct) -> syn::Result<TokenStream> {
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
    let abi = format!("{{\"version\":{version}}}");
    Ok(quote! {
        #item

        impl ::calimero_app_sdk::app::AppState for #name {
            const VERSION: u32 = #version;
            const ABI: &'static str = #abi;

            fn load() -> Self {
                Self { #(#inits),* }
            }
        }
    })
}
