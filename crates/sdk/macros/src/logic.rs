use proc_macro2::Span;
use quote::{quote, quote_spanned, ToTokens};

use crate::errors;

mod arg;
mod method;
mod ty;
mod utils;

pub struct LogicImpl<'a> {
    type_: &'a syn::Path,
    methods: Vec<method::PublicLogicMethod<'a>>,
    orig: &'a syn::ItemImpl,
}

impl<'a> ToTokens for LogicImpl<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let LogicImpl {
            type_: _,
            orig,
            methods,
        } = self;

        let guard = quote_spanned! {Span::call_site()=>
            // #[cfg(not(any(test, target_arch = "wasm32")))]
            // compile_error!(
            //     "incompatible target architecture, no polyfill available, only wasm32 is supported."
            // );
        };

        quote! {
            #guard

            #orig

            #(#methods)*
        }
        .to_tokens(tokens)
    }
}

impl<'a> TryFrom<&'a syn::ItemImpl> for LogicImpl<'a> {
    type Error = errors::Errors<'a, syn::ItemImpl>;

    fn try_from(item: &'a syn::ItemImpl) -> Result<Self, Self::Error> {
        let mut errors = errors::Errors::new(item);

        if let Some(_) = &item.trait_ {
            return Err(errors.finish(item, errors::ParseError::NoTraitSupport));
        }

        let Some(type_) = utils::typed_path(item.self_ty.as_ref(), false) else {
            return Err(errors.finish(&item.self_ty, errors::ParseError::UnsupportedImplType));
        };

        for generic in &item.generics.params {
            if let syn::GenericParam::Lifetime(_) = generic {
                continue;
            }
            errors.push(generic, errors::ParseError::NoGenericSupport);
        }

        let mut methods = vec![];
        for item in &item.items {
            if let syn::ImplItem::Fn(method) = item {
                match (type_, method).try_into() {
                    Ok(method::LogicMethod::Private) => {}
                    Ok(method::LogicMethod::Public(method)) => methods.push(method),
                    Err(err) => errors = errors.subsume(err),
                }
            }
        }

        errors.check(Self {
            type_,
            methods,
            orig: item,
        })
    }
}
