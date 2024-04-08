use quote::{quote, ToTokens};

use super::ty::LogicTy;
use super::utils;
use crate::errors;

pub enum Reference {
    Mutable,
    Immutable,
}

pub enum LogicArg<'a> {
    Receiver(Option<Reference>),
    Typed { ident: &'a syn::Ident, ty: LogicTy },
}

impl<'a> ToTokens for LogicArg<'a> {
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

impl<'a> TryFrom<(&'a syn::Path, &'a syn::FnArg)> for LogicArg<'a> {
    type Error = errors::Errors<'a, syn::FnArg>;

    fn try_from((type_, arg): (&'a syn::Path, &'a syn::FnArg)) -> Result<Self, Self::Error> {
        let mut errors = errors::Errors::new(arg);

        match arg {
            syn::FnArg::Receiver(receiver) => {
                'recv: {
                    // dbg!(&receiver.attrs);

                    let Some(path) = utils::typed_path(&receiver.ty, true) else {
                        break 'recv;
                    };

                    let is_self = type_ == path || path.is_ident("Self");

                    let mut reference = None;

                    if let syn::Type::Reference(ref_) = &*receiver.ty {
                        dbg!(&receiver.mutability);
                        dbg!(&ref_.mutability);
                        reference = ref_
                            .mutability
                            // receiver.mutability
                            .map_or(Some(Reference::Immutable), |_| Some(Reference::Mutable));
                    } else if is_self {
                        // todo! circumvent via `#[app::destroy]`
                        errors.push(&receiver.ty, errors::ParseError::NoSelfOwnership);
                    }

                    if is_self {
                        return errors.check(Self::Receiver(reference));
                    }
                };

                Err(errors.finish(
                    &receiver.ty,
                    errors::ParseError::ExpectedSelf(errors::Pretty::Path(type_)),
                ))
            }
            syn::FnArg::Typed(typed) => {
                let syn::Pat::Ident(ident) = &*typed.pat else {
                    return Err(errors.finish(&typed.pat, errors::ParseError::ExpectedIdent));
                };

                let ty = match (type_, &*typed.ty).try_into() {
                    Ok(ty) => ty,
                    Err(err) => return Err(errors.subsume(err)),
                };

                errors.check(Self::Typed {
                    ident: &ident.ident,
                    ty,
                })
            }
        }
    }
}
