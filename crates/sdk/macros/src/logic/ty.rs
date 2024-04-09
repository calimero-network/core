use quote::ToTokens;

use crate::errors;
use crate::sanitizer;

pub struct LogicTy {
    pub ty: syn::Type,
    pub ref_: bool,
}

impl ToTokens for LogicTy {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        self.ty.to_tokens(tokens)
    }
}

pub struct LogicTyInput<'a> {
    pub type_: &'a syn::Path,
    pub lifetime: &'a syn::Lifetime,
    pub ty: &'a syn::Type,
}

impl<'a> TryFrom<LogicTyInput<'a>> for LogicTy {
    type Error = errors::Errors<'a, syn::Type>;

    fn try_from(input: LogicTyInput<'a>) -> Result<Self, Self::Error> {
        let errors = errors::Errors::new(input.ty);

        'fatal: {
            let Ok(sanitizer) = syn::parse2::<sanitizer::Sanitizer>(input.ty.to_token_stream())
            else {
                break 'fatal;
            };

            let sanitizer = sanitizer
                .with_self(input.type_)
                .with_lifetime(&input.lifetime);

            let Ok(ty) = syn::parse2(sanitizer.to_token_stream()) else {
                break 'fatal;
            };

            return errors.check(LogicTy {
                ty,
                ref_: sanitizer.metrics().lifetimes > 0,
            });
        };

        return Err(errors.finish(input.ty, errors::ParseError::SanitizationFailed));
    }
}
