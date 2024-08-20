use quote::{quote, ToTokens};

use crate::{errors, items, reserved};

pub struct EventImpl<'a> {
    ident: &'a syn::Ident,
    generics: &'a syn::Generics,
    orig: &'a items::StructOrEnumItem,
}

impl ToTokens for EventImpl<'_> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
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
                .push(syn::parse_quote!(#generic_ty: ::calimero_sdk::serde::Serialize));
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
        .to_tokens(tokens)
    }
}

pub struct EventImplInput<'a> {
    pub item: &'a items::StructOrEnumItem,
}

impl<'a> TryFrom<EventImplInput<'a>> for EventImpl<'a> {
    type Error = errors::Errors<'a, items::StructOrEnumItem>;

    fn try_from(input: EventImplInput<'a>) -> Result<Self, Self::Error> {
        let mut errors = errors::Errors::new(input.item);

        let (vis, ident, generics) = match input.item {
            items::StructOrEnumItem::Struct(item) => (&item.vis, &item.ident, &item.generics),
            items::StructOrEnumItem::Enum(item) => (&item.vis, &item.ident, &item.generics),
        };

        match vis {
            syn::Visibility::Public(_) => {}
            syn::Visibility::Inherited => {
                return Err(errors.finish(syn::Error::new_spanned(
                    ident,
                    errors::ParseError::NoPrivateEvent,
                )));
            }
            syn::Visibility::Restricted(spec) => {
                return Err(errors.finish(syn::Error::new_spanned(
                    spec,
                    errors::ParseError::NoComplexVisibility,
                )));
            }
        }

        if ident == &*reserved::idents::input() {
            errors.subsume(syn::Error::new_spanned(
                ident,
                errors::ParseError::UseOfReservedIdent,
            ));
        }

        for generic in &generics.params {
            match generic {
                syn::GenericParam::Lifetime(params) => {
                    if params.lifetime == *reserved::lifetimes::input() {
                        errors.subsume(syn::Error::new(
                            params.lifetime.span(),
                            errors::ParseError::UseOfReservedLifetime,
                        ));
                    }
                }
                syn::GenericParam::Type(params) => {
                    if params.ident == *reserved::idents::input() {
                        errors.subsume(syn::Error::new_spanned(
                            &params.ident,
                            errors::ParseError::UseOfReservedIdent,
                        ));
                    }
                }
                syn::GenericParam::Const(_) => {}
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
