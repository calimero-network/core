use quote::{quote, ToTokens};

use crate::{errors, items, reserved};

#[derive(Copy, Clone)]
pub struct StateImpl<'a> {
    ident: &'a syn::Ident,
    generics: &'a syn::Generics,
    orig: &'a items::StructOrEnumItem,
}

impl<'a> ToTokens for StateImpl<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let StateImpl {
            ident,
            generics,
            orig,
        } = *self;

        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        let traits = [
            quote! { ::calimero_sdk::__private::NotQuiteSealedButStillPrivate },
            quote! { ::calimero_sdk::marker::AppState },
        ];

        quote! {
            #orig

            #(
                impl #impl_generics #traits for #ident #ty_generics #where_clause {}
            )*
        }
        .to_tokens(tokens)
    }
}

pub struct StateImplInput<'a> {
    pub item: &'a items::StructOrEnumItem,
}

impl<'a> TryFrom<StateImplInput<'a>> for StateImpl<'a> {
    type Error = errors::Errors<'a, items::StructOrEnumItem>;

    fn try_from(input: StateImplInput<'a>) -> Result<Self, Self::Error> {
        let mut errors = errors::Errors::new(input.item);

        let (ident, generics) = match input.item {
            items::StructOrEnumItem::Struct(item) => (&item.ident, &item.generics),
            items::StructOrEnumItem::Enum(item) => (&item.ident, &item.generics),
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
