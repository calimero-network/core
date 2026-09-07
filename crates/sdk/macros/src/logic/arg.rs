use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{Error as SynError, FnArg, Ident, Pat, Path, Receiver, ReceiverKind, Type};

use crate::errors::{Errors, ParseError, Pretty};
use crate::logic::ty::{LogicTy, LogicTyInput};
use crate::logic::utils::typed_path;

/// A method's `self` receiver, carrying the tokens a diagnostic about it should
/// point at.
pub enum SelfType {
    Owned(TokenStream),
    Mutable(TokenStream),
    Immutable(TokenStream),
}

impl SelfType {
    fn by_ref(mutable: bool, span: TokenStream) -> Self {
        if mutable {
            Self::Mutable(span)
        } else {
            Self::Immutable(span)
        }
    }
}

/// syn models a shorthand receiver without a type, so span what the author wrote:
/// the ascribed type of `self: T`, else the receiver itself minus a leading `mut`.
fn receiver_tokens(receiver: &Receiver) -> TokenStream {
    let self_token = &receiver.self_token;

    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "`ReceiverKind` is `#[non_exhaustive]`"
    )]
    match &receiver.kind {
        ReceiverKind::Typed(_, ty) => ty.to_token_stream(),
        ReceiverKind::Reference(ampersand, lifetime, mutability) => {
            quote! { #ampersand #lifetime #mutability #self_token }
        }
        _ => self_token.to_token_stream(),
    }
}

pub enum LogicArg<'a> {
    Receiver(SelfType),
    Typed(Box<LogicArgTyped<'a>>),
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
                let span = receiver_tokens(receiver);

                'recv: {
                    // `self`, `&self` and `&mut self` name the impl type by
                    // construction; only `self: T` has to be matched against it.
                    #[expect(
                        clippy::wildcard_enum_match_arm,
                        reason = "`ReceiverKind` is `#[non_exhaustive]`"
                    )]
                    let (is_self, reference) = match &receiver.kind {
                        ReceiverKind::Reference(_, _, mutability) => (
                            true,
                            Some(SelfType::by_ref(mutability.is_some(), span.clone())),
                        ),
                        ReceiverKind::Typed(_, ty) => {
                            let Some(path) = typed_path(ty, true) else {
                                break 'recv;
                            };

                            let mut reference = None;

                            if let Type::Reference(ref_) = &**ty {
                                reference =
                                    Some(SelfType::by_ref(ref_.mutability.is_some(), span.clone()));
                            }

                            (input.type_ == path || path.is_ident("Self"), reference)
                        }
                        _ => (true, None),
                    };

                    if reference.is_none() && is_self {
                        // todo! circumvent via `#[app::destroy]`
                        errors.subsume(SynError::new_spanned(&span, ParseError::NoSelfOwnership));
                    }

                    if is_self {
                        errors.check()?;

                        return Ok(Self::Receiver(reference.unwrap_or(SelfType::Owned(span))));
                    }
                }

                Err(errors.finish(SynError::new_spanned(
                    &span,
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

                Ok(LogicArg::Typed(Box::new(LogicArgTyped {
                    ident: &ident.ident,
                    ty,
                })))
            }
        }
    }
}
