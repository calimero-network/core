use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{Error as SynError, FnArg, Ident, Pat, Path, Type};

use crate::errors::{Errors, ParseError, Pretty};
use crate::logic::ty::{LogicTy, LogicTyInput};
use crate::logic::utils::typed_path;

pub enum SelfType<'a> {
    Owned(&'a Type),
    Mutable(&'a Type),
    Immutable(&'a Type),
}

pub enum LogicArg<'a> {
    Receiver(SelfType<'a>),
    Typed(LogicArgTyped<'a>),
}

pub struct LogicArgTyped<'a> {
    pub ident: &'a Ident,
    pub ty: LogicTy,
}

impl ToTokens for LogicArgTyped<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ident = &self.ident;
        let ty = &self.ty;

        quote! { #ident: #ty }.to_tokens(tokens);
    }
}

pub struct LogicArgInput<'a, 'b> {
    pub arg: &'a FnArg,

    pub type_: &'b Path,
}

impl<'a, 'b> TryFrom<LogicArgInput<'a, 'b>> for LogicArg<'a> {
    type Error = Errors<'a, FnArg>;

    fn try_from(input: LogicArgInput<'a, 'b>) -> Result<Self, Self::Error> {
        let errors = Errors::new(input.arg);

        match input.arg {
            FnArg::Receiver(receiver) => {
                'recv: {
                    let Some(path) = typed_path(&receiver.ty, true) else {
                        break 'recv;
                    };

                    let is_self = input.type_ == path || path.is_ident("Self");

                    let mut reference = None;

                    if let Type::Reference(ref_) = &*receiver.ty {
                        reference = ref_
                            .mutability
                            .map_or(Some(SelfType::Immutable(&receiver.ty)), |_| {
                                Some(SelfType::Mutable(&receiver.ty))
                            });
                    } else if is_self {
                        // todo! circumvent via `#[app::destroy]`
                        errors.subsume(SynError::new_spanned(
                            &receiver.ty,
                            ParseError::NoSelfOwnership,
                        ));
                    }

                    if is_self {
                        errors.check()?;

                        return Ok(Self::Receiver(
                            reference.unwrap_or(SelfType::Owned(&receiver.ty)),
                        ));
                    }
                }

                Err(errors.finish(SynError::new_spanned(
                    &receiver.ty,
                    ParseError::ExpectedSelf(Pretty::Path(input.type_)),
                )))
            }
            FnArg::Typed(typed) => {
                let Pat::Ident(ident) = &*typed.pat else {
                    return Err(
                        errors.finish(SynError::new_spanned(&typed.pat, ParseError::ExpectedIdent))
                    );
                };

                let ty = match LogicTy::try_from(LogicTyInput {
                    type_: input.type_,
                    ty: &typed.ty,
                }) {
                    Ok(ty) => ty,
                    Err(err) => {
                        errors.combine(&err);
                        return Err(errors);
                    }
                };

                errors.check()?;

                Ok(LogicArg::Typed(LogicArgTyped {
                    ident: &ident.ident,
                    ty,
                }))
            }
        }
    }
}
