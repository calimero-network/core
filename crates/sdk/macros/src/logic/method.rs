use quote::{quote, ToTokens};

use super::arg::LogicArg;
use crate::errors;

pub enum LogicMethod<'a> {
    Public(PublicLogicMethod<'a>),
    Private,
}

pub struct PublicLogicMethod<'a> {
    name: &'a syn::Ident,
    mutable: bool,
    args: Vec<LogicArg<'a>>,
    // ret: syn::Type,
    // orig: &'a syn::ImplItemFn,
}

impl<'a> ToTokens for PublicLogicMethod<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        // let ident =
        quote! {
            // #[cfg(target_arch = "wasm32")]
            // #[no_mangle]
            // pub extern "C" fn
        }
        .to_tokens(tokens)
    }
}

impl<'a> TryFrom<(&'a syn::Path, &'a syn::ImplItemFn)> for LogicMethod<'a> {
    type Error = errors::Errors<'a, syn::ImplItemFn>;

    fn try_from((type_, item): (&'a syn::Path, &'a syn::ImplItemFn)) -> Result<Self, Self::Error> {
        // dbg!(&item.attrs);

        if !matches!(item.vis, syn::Visibility::Public(_)) {
            return Ok(Self::Private);
        }

        let mut errors = errors::Errors::new(item);

        if let Some(asyncness) = &item.sig.asyncness {
            errors.push(asyncness, errors::ParseError::NoAsyncSupport);
        }

        if let Some(unsafety) = &item.sig.unsafety {
            errors.push(unsafety, errors::ParseError::NoUnsafeSupport);
        }

        for generic in &item.sig.generics.params {
            if let syn::GenericParam::Lifetime(_) = generic {
                continue;
            }
            errors.push(generic, errors::ParseError::NoGenericSupport);
        }

        // todo! test if generics break codegen

        let mut has_ref = false;

        let mut args = vec![];
        for arg in &item.sig.inputs {
            match (type_, arg).try_into() {
                Ok(arg) => args.push(arg),
                Err(err) => errors = errors.subsume(err),
            }
        }

        errors.check(LogicMethod::Public(PublicLogicMethod {
            name: &item.sig.ident,
            args,
            mutable: true, // todo! depends on args to be determined
                           // ret,
                           // orig: item,
        }))
    }
}
