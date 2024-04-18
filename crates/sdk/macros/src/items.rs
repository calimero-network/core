use quote::ToTokens;
use syn::parse::{Parse, ParseStream};

use crate::errors;

pub enum StructOrEnumItem {
    Struct(syn::ItemStruct),
    Enum(syn::ItemEnum),
}

impl Parse for StructOrEnumItem {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut attrs = Vec::new();
        let mut vis = syn::Visibility::Inherited;
        'parsed: loop {
            let lookahead = input.lookahead1();
            if lookahead.peek(syn::Token![struct]) {
                break 'parsed input
                    .parse()
                    .map(|s| StructOrEnumItem::Struct(syn::ItemStruct { attrs, vis, ..s }));
            } else if lookahead.peek(syn::Token![enum]) {
                break 'parsed input
                    .parse()
                    .map(|s| StructOrEnumItem::Enum(syn::ItemEnum { attrs, vis, ..s }));
            } else if lookahead.peek(syn::Token![#]) {
                attrs.extend(input.call(syn::Attribute::parse_outer)?);
            } else if lookahead.peek(syn::Token![pub]) {
                vis = input.parse::<syn::Visibility>()?;
            } else {
                let err = lookahead.error();

                return Err(syn::Error::new(
                    err.span(),
                    errors::ParseError::Custom(&err.to_string()).to_string(),
                ));
            }
        }
    }
}

impl ToTokens for StructOrEnumItem {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        match self {
            StructOrEnumItem::Struct(item) => item.to_tokens(tokens),
            StructOrEnumItem::Enum(item) => item.to_tokens(tokens),
        }
    }
}
