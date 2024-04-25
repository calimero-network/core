use quote::ToTokens;

use crate::{errors, reserved, sanitizer};

pub struct LogicTy {
    pub ty: syn::Type,
    pub ref_: bool,
}

impl ToTokens for LogicTy {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        self.ty.to_tokens(tokens)
    }
}

pub struct LogicTyInput<'a, 'b> {
    pub ty: &'a syn::Type,

    pub type_: &'b syn::Path,
}

impl<'a, 'b> TryFrom<LogicTyInput<'a, 'b>> for LogicTy {
    type Error = errors::Errors<'a, syn::Type>;

    fn try_from(input: LogicTyInput<'a, 'b>) -> Result<Self, Self::Error> {
        let mut errors = errors::Errors::new(input.ty);

        'fatal: {
            let Ok(mut sanitizer) = syn::parse2::<sanitizer::Sanitizer>(input.ty.to_token_stream())
            else {
                break 'fatal;
            };

            let reserved_ident = reserved::idents::input();
            let reserved_lifetime = reserved::lifetimes::input();

            let cases = [
                (
                    sanitizer::Case::Self_,
                    sanitizer::Action::ReplaceWith(&input.type_),
                ),
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
                    sanitizer::Action::ReplaceWith(&reserved_lifetime),
                ),
            ];

            let outcome = sanitizer.sanitize(&cases);

            if let Err(err) = outcome.check() {
                errors.subsume(err);
            }

            let has_ref = matches!(
                (
                    outcome.count(&sanitizer::Case::Lifetime(None)),
                    outcome.count(&sanitizer::Case::Lifetime(Some(&reserved_lifetime)))
                ),
                (1.., _) | (_, 1..)
            );

            let Ok(ty) = syn::parse2(sanitizer.to_token_stream()) else {
                break 'fatal;
            };

            errors.check()?;

            return Ok(LogicTy { ty, ref_: has_ref });
        };

        Err(errors.finish(syn::Error::new_spanned(
            input.ty,
            errors::ParseError::SanitizationFailed,
        )))
    }
}
