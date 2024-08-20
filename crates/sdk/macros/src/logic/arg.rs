use quote::{quote, ToTokens};

use super::{ty, utils};
use crate::errors;

pub enum SelfType<'a> {
    Owned(&'a syn::Type),
    Mutable(&'a syn::Type),
    Immutable(&'a syn::Type),
}

pub enum LogicArg<'a> {
    Receiver(SelfType<'a>),
    Typed(LogicArgTyped<'a>),
}

pub struct LogicArgTyped<'a> {
    pub ident: &'a syn::Ident,
    pub ty: ty::LogicTy,
}

impl<'a> ToTokens for LogicArgTyped<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let ident = &self.ident;
        let ty = &self.ty;

        quote! { #ident: #ty }.to_tokens(tokens)
    }
}

pub struct LogicArgInput<'a, 'b> {
    pub arg: &'a syn::FnArg,

    pub type_: &'b syn::Path,
}

impl<'a, 'b> TryFrom<LogicArgInput<'a, 'b>> for LogicArg<'a> {
    type Error = errors::Errors<'a, syn::FnArg>;

    fn try_from(input: LogicArgInput<'a, 'b>) -> Result<Self, Self::Error> {
        let mut errors = errors::Errors::new(input.arg);

        match input.arg {
            syn::FnArg::Receiver(receiver) => {
                'recv: {
                    let Some(path) = utils::typed_path(&receiver.ty, true) else {
                        break 'recv;
                    };

                    let is_self = input.type_ == path || path.is_ident("Self");

                    let mut reference = None;

                    if let syn::Type::Reference(ref_) = &*receiver.ty {
                        reference = ref_
                            .mutability
                            .map_or(Some(SelfType::Immutable(&*receiver.ty)), |_| {
                                Some(SelfType::Mutable(&*receiver.ty))
                            });
                    } else if is_self {
                        // todo! circumvent via `#[app::destroy]`
                        errors.subsume(syn::Error::new_spanned(
                            &receiver.ty,
                            errors::ParseError::NoSelfOwnership,
                        ));
                    }

                    if is_self {
                        errors.check()?;

                        return Ok(Self::Receiver(
                            reference.unwrap_or(SelfType::Owned(&*receiver.ty)),
                        ));
                    }
                };

                Err(errors.finish(syn::Error::new_spanned(
                    &receiver.ty,
                    errors::ParseError::ExpectedSelf(errors::Pretty::Path(input.type_)),
                )))
            }
            syn::FnArg::Typed(typed) => {
                let syn::Pat::Ident(ident) = &*typed.pat else {
                    return Err(errors.finish(syn::Error::new_spanned(
                        &typed.pat,
                        errors::ParseError::ExpectedIdent,
                    )));
                };

                let ty = match ty::LogicTy::try_from(ty::LogicTyInput {
                    type_: input.type_,
                    ty: &typed.ty,
                }) {
                    Ok(ty) => ty,
                    Err(err) => {
                        errors.combine(err);
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
