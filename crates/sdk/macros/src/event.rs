use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{parse_quote, Error as SynError, GenericParam, Generics, Ident, Visibility};

use crate::errors::{Errors, ParseError};
use crate::items::StructOrEnumItem;
use crate::reserved::{idents, lifetimes};

pub struct EventImpl<'a> {
    ident: &'a Ident,
    generics: &'a Generics,
    orig: &'a StructOrEnumItem,
}

impl ToTokens for EventImpl<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let EventImpl {
            ident,
            generics: source_generics,
            orig,
        } = *self;

        let mut generics = source_generics.clone();

        for generic_ty in source_generics.type_params() {
            generics
                .make_where_clause()
                .predicates
                .push(parse_quote!(#generic_ty: ::calimero_sdk::serde::Serialize));
        }

        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        quote! {
            #[derive(::calimero_sdk::serde::Serialize)]
            #[serde(crate = "::calimero_sdk::serde")]
            #[serde(tag = "kind", content = "data")]
            #orig

            impl #impl_generics ::calimero_sdk::event::AppEvent for #ident #ty_generics #where_clause {
                fn kind(&self) -> ::std::borrow::Cow<str> {
                    // todo! revisit quick
                    match ::calimero_sdk::serde_json::to_value(self) {
                        Ok(data) => ::std::borrow::Cow::Owned(data["kind"].as_str().expect("Failed to get event kind").to_string()),
                        Err(err) => ::calimero_sdk::env::panic_str(
                            &format!("Failed to serialize event: {:?}", err)
                        ),
                    }
                }
                fn data(&self) -> ::std::borrow::Cow<[u8]> {
                    // todo! revisit quick
                    match ::calimero_sdk::serde_json::to_value(self) {
                        Ok(data) => ::std::borrow::Cow::Owned(::calimero_sdk::serde_json::to_vec(&data["data"]).expect("Failed to serialize event data")),
                        Err(err) => ::calimero_sdk::env::panic_str(
                            &format!("Failed to serialize event: {:?}", err)
                        ),
                    }
                }
            }

            impl #impl_generics ::calimero_sdk::event::AppEventExt for #ident #ty_generics #where_clause {}
        }
        .to_tokens(tokens);
    }
}

pub struct EventImplInput<'a> {
    pub item: &'a StructOrEnumItem,
}

impl<'a> TryFrom<EventImplInput<'a>> for EventImpl<'a> {
    type Error = Errors<'a, StructOrEnumItem>;

    fn try_from(input: EventImplInput<'a>) -> Result<Self, Self::Error> {
        let errors = Errors::new(input.item);

        let (vis, ident, generics) = match input.item {
            StructOrEnumItem::Struct(item) => (&item.vis, &item.ident, &item.generics),
            StructOrEnumItem::Enum(item) => (&item.vis, &item.ident, &item.generics),
        };

        match vis {
            Visibility::Public(_) => {}
            Visibility::Inherited => {
                return Err(errors.finish(SynError::new_spanned(ident, ParseError::NoPrivateEvent)));
            }
            Visibility::Restricted(spec) => {
                return Err(
                    errors.finish(SynError::new_spanned(spec, ParseError::NoComplexVisibility))
                );
            }
        }

        if ident == &*idents::input() {
            errors.subsume(SynError::new_spanned(ident, ParseError::UseOfReservedIdent));
        }

        for generic in &generics.params {
            match generic {
                GenericParam::Lifetime(params) => {
                    if params.lifetime == *lifetimes::input() {
                        errors.subsume(SynError::new(
                            params.lifetime.span(),
                            ParseError::UseOfReservedLifetime,
                        ));
                    }
                }
                GenericParam::Type(params) => {
                    if params.ident == *idents::input() {
                        errors.subsume(SynError::new_spanned(
                            &params.ident,
                            ParseError::UseOfReservedIdent,
                        ));
                    }
                }
                GenericParam::Const(_) => {}
            }
        }

        errors.check()?;

        Ok(EventImpl {
            ident,
            generics,
            orig: input.item,
        })
    }
}
