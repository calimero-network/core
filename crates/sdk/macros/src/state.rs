use quote::{quote, ToTokens};
use syn::parse::Parse;

use crate::{errors, reserved};

pub enum StateItem {
    Struct(syn::ItemStruct),
    Enum(syn::ItemEnum),
}

impl Parse for StateItem {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut attrs = Vec::new();
        'parsed: loop {
            let lookahead = input.lookahead1();
            if lookahead.peek(syn::Token![struct]) {
                break 'parsed input
                    .parse()
                    .map(|s| StateItem::Struct(syn::ItemStruct { attrs, ..s }));
            } else if lookahead.peek(syn::Token![enum]) {
                break 'parsed input
                    .parse()
                    .map(|s| StateItem::Enum(syn::ItemEnum { attrs, ..s }));
            } else if lookahead.peek(syn::Token![#]) {
                attrs.extend(input.call(syn::Attribute::parse_outer)?);
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

impl ToTokens for StateItem {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        match self {
            StateItem::Struct(item) => item.to_tokens(tokens),
            StateItem::Enum(item) => item.to_tokens(tokens),
        }
    }
}

#[derive(Copy, Clone)]
pub struct StateImpl<'a> {
    ident: &'a syn::Ident,
    generics: &'a syn::Generics,
    orig: &'a StateItem,
}

impl<'a> ToTokens for StateImpl<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let StateImpl {
            ident,
            generics,
            orig,
        } = *self;

        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        quote! {
            #orig

            impl #impl_generics ::calimero_sdk::__private::marker::AppState for #ident #ty_generics #where_clause {}
        }
        .to_tokens(tokens)
    }
}

pub struct StateImplInput<'a> {
    pub item: &'a StateItem,
}

impl<'a> TryFrom<StateImplInput<'a>> for StateImpl<'a> {
    type Error = errors::Errors<'a, StateItem>;

    fn try_from(input: StateImplInput<'a>) -> Result<Self, Self::Error> {
        let mut errors = errors::Errors::new(input.item);

        let (ident, generics) = match input.item {
            StateItem::Struct(item) => (&item.ident, &item.generics),
            StateItem::Enum(item) => (&item.ident, &item.generics),
        };

        if ident == &*reserved::idents::input() {
            errors.push_spanned(&ident, errors::ParseError::UseOfReservedIdent);
        }

        for generic in &generics.params {
            match generic {
                syn::GenericParam::Lifetime(params) => {
                    errors.push(
                        params.lifetime.span(),
                        errors::ParseError::NoGenericLifetimeSupport,
                    );
                    continue;
                }
                syn::GenericParam::Type(params) => {
                    if params.ident == *reserved::idents::input() {
                        errors.push_spanned(&params.ident, errors::ParseError::UseOfReservedIdent);
                    }
                }
                syn::GenericParam::Const(_) => {}
            }
        }

        errors.check(StateImpl {
            ident,
            generics,
            orig: input.item,
        })
    }
}
