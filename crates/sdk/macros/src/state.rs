use quote::{quote, ToTokens};
use syn::parse::Parse;

use crate::{errors, items, reserved, sanitizer};

#[derive(Copy, Clone)]
pub struct StateImpl<'a> {
    ident: &'a syn::Ident,
    generics: &'a syn::Generics,
    emits: &'a Option<MaybeBoundEvent>,
    orig: &'a items::StructOrEnumItem,
}

impl<'a> ToTokens for StateImpl<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let StateImpl {
            ident,
            generics,
            emits,
            orig,
        } = *self;

        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        let mut lifetime = quote! { 'a };
        let mut event = quote! { () };

        if let Some(emits) = emits {
            if let Some(lt) = &emits.lifetime {
                lifetime = quote! { #lt };
            }
            event = {
                let event = &emits.path;
                quote! { #event }
            };
        }

        quote! {
            #orig

            impl #impl_generics ::calimero_sdk::marker::AppState for #ident #ty_generics #where_clause {
                type Event<#lifetime> = #event;
            }
        }
        .to_tokens(tokens)
    }
}

struct MaybeBoundEvent {
    lifetime: Option<syn::Lifetime>,
    path: syn::Path,
}

impl syn::parse::Parse for MaybeBoundEvent {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut lifetime = None;

        if input.peek(syn::Token![for]) {
            let syn::BoundLifetimes { lifetimes, .. } = input.parse()?;

            for param in lifetimes {
                if let syn::GenericParam::Lifetime(syn::LifetimeParam { lifetime: lt, .. }) = param
                {
                    if lifetime.is_some() {
                        return Err(syn::Error::new(
                            lt.span(),
                            errors::ParseError::Custom("only one lifetime can be specified"),
                        ));
                    } else {
                        lifetime = Some(lt);
                    }
                }
            }
        };

        let path = input.parse::<syn::Path>()?;

        let mut sanitizer = syn::parse2::<sanitizer::Sanitizer>(path.to_token_stream())?;

        let mut cases = vec![];

        if let Some(lt) = &lifetime {
            cases.push((
                sanitizer::Case::Lifetime(Some(lt)),
                sanitizer::Action::Ignore,
            ));
        }

        let static_lifetime = syn::Lifetime::new("'static", proc_macro2::Span::call_site());

        let mut unexpected_lifetime = |span: proc_macro2::Span| {
            sanitizer::Action::Forbid(errors::ParseError::UseOfUndeclaredLifetime {
                append: format!(
                    "\n\nuse the `for<{}> {}` directive to declare it",
                    span.source_text()
                        .unwrap_or_else(|| "'{lifetime}".to_owned()),
                    errors::Pretty::Path(&path)
                ),
            })
        };

        cases.extend([
            (
                sanitizer::Case::Lifetime(Some(&static_lifetime)),
                sanitizer::Action::Ignore,
            ),
            (
                sanitizer::Case::Lifetime(None),
                sanitizer::Action::Custom(sanitizer::Func::new(&mut unexpected_lifetime)),
            ),
        ]);

        let mut outcome = sanitizer.sanitize(&cases);

        if let Some(lifetime) = &lifetime {
            if 0 == outcome.count(&sanitizer::Case::Lifetime(Some(lifetime))) {
                outcome.errors().push(
                    lifetime.span(),
                    errors::ParseError::Custom("unused lifetime specified"),
                );
            }
        }

        outcome.check()?;

        let path = syn::parse2(sanitizer.into_token_stream())?;

        input
            .is_empty()
            .then(|| MaybeBoundEvent { lifetime, path })
            .ok_or_else(|| input.error("unexpected token"))
    }
}

pub struct StateArgs {
    emits: Option<MaybeBoundEvent>,
}

impl Parse for StateArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut emits = None;

        if !input.is_empty() {
            let ident = input.parse::<syn::Ident>()?;

            input.parse::<syn::Token![=]>()?;

            if ident == "emits" {
                emits = Some(input.parse::<MaybeBoundEvent>()?);
            }

            if !input.is_empty() {
                return Err(input.error("unexpected token"));
            }
        }

        Ok(StateArgs { emits })
    }
}

pub struct StateImplInput<'a> {
    pub item: &'a items::StructOrEnumItem,
    pub args: &'a StateArgs,
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
            emits: &input.args.emits,
            orig: input.item,
        })
    }
}
