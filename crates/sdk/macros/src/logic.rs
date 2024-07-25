use quote::{quote, ToTokens};

use crate::macros::infallible;
use crate::{errors, reserved, sanitizer};

mod arg;
mod method;
mod ty;
mod utils;

pub struct LogicImpl<'a> {
    methods: Vec<method::PublicLogicMethod<'a>>,
    orig: &'a syn::ItemImpl,
}

impl<'a> ToTokens for LogicImpl<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let LogicImpl { orig, methods, .. } = self;

        quote! {
            #orig

            #(#methods)*

        }
        .to_tokens(tokens)
    }
}

pub struct LogicImplInput<'a> {
    pub item: &'a syn::ItemImpl,
}

impl<'a> TryFrom<LogicImplInput<'a>> for LogicImpl<'a> {
    type Error = errors::Errors<'a, syn::ItemImpl>;

    fn try_from(input: LogicImplInput<'a>) -> Result<Self, Self::Error> {
        let mut errors = errors::Errors::new(input.item);

        for generic in &input.item.generics.params {
            if let syn::GenericParam::Lifetime(params) = generic {
                if params.lifetime == *reserved::lifetimes::input() {
                    errors.subsume(syn::Error::new(
                        params.lifetime.span(),
                        errors::ParseError::UseOfReservedLifetime,
                    ));
                }
                continue;
            }
            errors.subsume(syn::Error::new_spanned(
                generic,
                errors::ParseError::NoGenericTypeSupport,
            ));
        }

        if let Some(_) = &input.item.trait_ {
            return Err(errors.finish(syn::Error::new_spanned(
                input.item,
                errors::ParseError::NoTraitSupport,
            )));
        }

        let Some(type_) = utils::typed_path(input.item.self_ty.as_ref(), false) else {
            return Err(errors.finish(syn::Error::new_spanned(
                &input.item.self_ty,
                errors::ParseError::UnsupportedImplType,
            )));
        };

        let mut sanitizer = syn::parse2::<sanitizer::Sanitizer>(type_.to_token_stream()).unwrap();

        let reserved_ident = reserved::idents::input();
        let reserved_lifetime = reserved::lifetimes::input();

        let cases = [
            (
                sanitizer::Case::Ident(Some(&reserved_ident)),
                sanitizer::Action::Forbid(errors::ParseError::UseOfReservedIdent),
            ),
            (
                sanitizer::Case::Lifetime(Some(&reserved_lifetime)),
                sanitizer::Action::Forbid(errors::ParseError::UseOfReservedLifetime),
            ),
            (
                sanitizer::Case::Lifetime(None),
                sanitizer::Action::Forbid(errors::ParseError::NoGenericLifetimeSupport),
            ),
        ];

        let outcome = sanitizer.sanitize(&cases);

        if let Err(err) = outcome.check() {
            errors.subsume(err);
        }

        if outcome.count(&sanitizer::Case::Ident(Some(&reserved_ident))) > 0 {
            // fail-fast due to reuse of the self ident for code generation
            return Err(errors);
        }

        let type_ = infallible!({ syn::parse2(sanitizer.to_token_stream()) });

        let mut methods = vec![];

        for item in &input.item.items {
            if let syn::ImplItem::Fn(method) = item {
                match method::LogicMethod::try_from(method::LogicMethodImplInput {
                    type_: &type_,
                    item: method,
                }) {
                    Ok(method::LogicMethod::Public(method)) => methods.push(method),
                    Ok(method::LogicMethod::Private) => {}
                    Err(err) => errors.combine(err),
                }
            }
        }

        if !methods.iter().any(|method| {
            method.attrs.iter().any(|attr| {
                attr.path().segments.len() == 2
                    && attr.path().segments[0].ident == "app"
                    && attr.path().segments[1].ident == "init"
            })
        }) {
            errors.subsume(syn::Error::new_spanned(
                input.item,
                "The #[app::init] method is not properly defined",
            ));
        }

        errors.check()?;

        Ok(Self {
            methods,
            orig: input.item,
        })
    }
}
