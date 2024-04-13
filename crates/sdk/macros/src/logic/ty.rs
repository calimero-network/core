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
    pub ty: &'a syn::Type,

    pub type_: &'a syn::Path,
    pub reserved_ident: &'a syn::Ident,
    pub reserved_lifetime: &'a syn::Lifetime,
}

impl<'a> TryFrom<LogicTyInput<'a>> for LogicTy {
    type Error = errors::Errors<'a, syn::Type>;

    fn try_from(input: LogicTyInput<'a>) -> Result<Self, Self::Error> {
        let mut errors = errors::Errors::new(input.ty);

        'fatal: {
            let Ok(mut sanitizer) = syn::parse2::<sanitizer::Sanitizer>(input.ty.to_token_stream())
            else {
                break 'fatal;
            };

            let cases = [
                (
                    sanitizer::Case::Self_,
                    sanitizer::Action::ReplaceWith(&input.type_),
                ),
                (
                    sanitizer::Case::Ident(Some(&input.reserved_ident)),
                    sanitizer::Action::Forbid(errors::ParseError::UseOfReservedIdent),
                ),
                (
                    sanitizer::Case::Lifetime(Some(&input.reserved_lifetime)),
                    sanitizer::Action::Forbid(errors::ParseError::UseOfReservedLifetime),
                ),
                (sanitizer::Case::Lifetime(None), sanitizer::Action::Ignore),
            ];

            let outcome = sanitizer.sanitize(&cases);

            if let Err(err) = outcome.check() {
                errors = errors.subsume(err);
            }

            let has_ref = matches!(
                (
                    outcome.get(&sanitizer::Case::Lifetime(None)),
                    outcome.get(&sanitizer::Case::Lifetime(Some(&input.reserved_lifetime)))
                ),
                (Some(_), _) | (_, Some(_))
            );

            let Ok(ty) = syn::parse2(sanitizer.to_token_stream()) else {
                break 'fatal;
            };

            return errors.check(LogicTy { ty, ref_: has_ref });
        };

        return Err(errors.finish(input.ty, errors::ParseError::SanitizationFailed));
    }
}
