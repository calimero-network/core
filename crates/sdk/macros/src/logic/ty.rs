use proc_macro2::TokenStream;
use quote::ToTokens;
use syn::{parse2, Path, Type};

use crate::errors::{Errors, ParseError};
use crate::macros::infallible;
use crate::reserved::{idents, lifetimes};
use crate::sanitizer::{Action, Case, Sanitizer};

pub struct LogicTy {
    pub ty: Type,
    pub ref_: bool,
}

impl ToTokens for LogicTy {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        self.ty.to_tokens(tokens);
    }
}

pub struct LogicTyInput<'a, 'b> {
    pub ty: &'a Type,

    pub type_: &'b Path,
}

impl<'a, 'b> TryFrom<LogicTyInput<'a, 'b>> for LogicTy {
    type Error = Errors<'a, Type>;

    // TODO: This unwrap() call needs to be corrected to return an error.
    #[expect(clippy::unwrap_in_result, reason = "TODO: This is temporary")]
    fn try_from(input: LogicTyInput<'a, 'b>) -> Result<Self, Self::Error> {
        let errors = Errors::new(input.ty);

        let mut sanitizer = parse2::<Sanitizer<'_>>(input.ty.to_token_stream()).unwrap();

        let reserved_ident = idents::input();
        let reserved_lifetime = lifetimes::input();

        let cases = [
            (Case::Self_, Action::ReplaceWith(&input.type_)),
            (
                Case::Ident(Some(&reserved_ident)),
                Action::Forbid(ParseError::UseOfReservedIdent),
            ),
            (
                Case::Lifetime(Some(&reserved_lifetime)),
                Action::Forbid(ParseError::UseOfReservedLifetime),
            ),
            (
                Case::Lifetime(None),
                Action::ReplaceWith(&reserved_lifetime),
            ),
        ];

        let outcome = sanitizer.sanitize(&cases);

        if let Err(err) = outcome.check() {
            errors.subsume(err);
        }

        let has_ref = matches!(
            (
                outcome.count(&Case::Lifetime(None)),
                outcome.count(&Case::Lifetime(Some(&reserved_lifetime)))
            ),
            (1.., _) | (_, 1..)
        );

        let ty = infallible!({ parse2(sanitizer.into_token_stream()) });

        errors.check()?;

        Ok(Self { ty, ref_: has_ref })
    }
}
