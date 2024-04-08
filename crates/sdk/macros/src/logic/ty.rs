use quote::{quote, ToTokens};

use super::utils;
use crate::errors;

pub struct LogicTy {
    ty: syn::Type,
    has_ref: bool,
}

impl ToTokens for LogicTy {
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

impl<'a> TryFrom<(&'a syn::Path, &'a syn::Type)> for LogicTy {
    type Error = errors::Errors<'a, syn::Type>;

    fn try_from((type_, ty): (&'a syn::Path, &'a syn::Type)) -> Result<Self, Self::Error> {
        let mut errors = errors::Errors::new(ty);

        for tt in ty.to_token_stream().into_iter() {
            dbg!(&tt);
        }

        let Ok(ty) = syn::parse2(utils::sanitize_self(ty, &type_.to_token_stream()).collect())
        else {
            return Err(errors.finish(ty, errors::ParseError::SelfSanitizationFailed));
        };

        // dbg!(&ty); // todo! test for deep refs

        // let mut has_ref = false;

        errors.check(LogicTy { ty, has_ref: false })
    }
}
