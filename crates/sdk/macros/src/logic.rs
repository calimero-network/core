use quote::{quote, ToTokens};

use crate::{errors, reserved};

mod arg;
mod method;
mod ty;
mod utils;

pub struct LogicImpl<'a> {
    #[allow(dead_code)]
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
            ..
        } = self;

        quote! {
            #[cfg(not(target_arch = "wasm32"))]
            compile_error!(
                "incompatible target architecture, no polyfill available, only wasm32 is supported."
            );

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

        if let Some(_) = &input.item.trait_ {
            return Err(errors.finish(input.item, errors::ParseError::NoTraitSupport));
        }

        let Some(type_) = utils::typed_path(input.item.self_ty.as_ref(), false) else {
            return Err(errors.finish(&input.item.self_ty, errors::ParseError::UnsupportedImplType));
        };

        for generic in &input.item.generics.params {
            if let syn::GenericParam::Lifetime(params) = generic {
                if params.lifetime == *reserved::lifetimes::input() {
                    errors
                        .push_spanned(&params.lifetime, errors::ParseError::UseOfReservedLifetime);
                }
                continue;
            }
            errors.push_spanned(generic, errors::ParseError::NoGenericSupport);
        }

        let mut methods = vec![];
        for item in &input.item.items {
            if let syn::ImplItem::Fn(method) = item {
                match method::LogicMethod::try_from(method::LogicMethodImplInput {
                    type_,
                    item: method,
                }) {
                    Ok(method::LogicMethod::Private) => {}
                    Ok(method::LogicMethod::Public(method)) => methods.push(method),
                    Err(err) => errors = errors.subsume(err),
                }
            }
        }

        errors.check(Self {
            type_,
            methods,
            orig: input.item,
        })
    }
}
