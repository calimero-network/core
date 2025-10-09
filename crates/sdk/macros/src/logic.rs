use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{parse2, Error as SynError, GenericParam, ImplItem, ItemImpl, Path};

use crate::errors::{Errors, ParseError};
use crate::logic::method::{CallbackMethod, LogicMethod, LogicMethodImplInput, PublicLogicMethod};
use crate::logic::utils::typed_path;
use crate::macros::infallible;
use crate::reserved::{idents, lifetimes};
use crate::sanitizer::{Action, Case, Sanitizer};

mod arg;
mod method;
mod ty;
mod utils;

pub struct LogicImpl<'a> {
    type_: Path,
    methods: Vec<PublicLogicMethod<'a>>,
    callbacks: Vec<CallbackMethod<'a>>,
    orig: &'a ItemImpl,
}

impl ToTokens for LogicImpl<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let LogicImpl { orig, methods, callbacks: _, type_: _ } = self;

        // Process callback methods
        let mut filtered_items = Vec::new();
        
        for item in &orig.items {
            match item {
                ImplItem::Fn(method) => {
                    // Check if this is a callback method
                    let callback_attr = method.attrs.iter().find(|attr| {
                        if let Some(ident) = attr.path().get_ident() {
                            return ident == "callback";
                        }
                        let segments: Vec<String> = attr
                            .path()
                            .segments
                            .iter()
                            .map(|s| s.ident.to_string())
                            .collect();
                        segments.as_slice() == ["app", "callback"]
                    });
                    
                    if let Some(_attr) = callback_attr {
                        // Include callback methods in the main impl block
                        // The #[app::callback] attribute handles its own registration
                        filtered_items.push(ImplItem::Fn(method.clone()));
                    } else {
                        filtered_items.push(ImplItem::Fn(method.clone()));
                    }
                }
                other => filtered_items.push(other.clone()),
            }
        }
        
        let filtered_impl = ItemImpl {
            attrs: orig.attrs.clone(),
            defaultness: orig.defaultness,
            unsafety: orig.unsafety,
            impl_token: orig.impl_token,
            generics: orig.generics.clone(),
            trait_: orig.trait_.clone(),
            self_ty: orig.self_ty.clone(),
            brace_token: orig.brace_token,
            items: filtered_items,
        };

        // Callbacks are now included in the main impl block, no separate impl needed


        quote! {
            #filtered_impl

            #(#methods)*
        }
        .to_tokens(tokens);
    }
}

pub struct LogicImplInput<'a> {
    pub item: &'a ItemImpl,
}

impl<'a> TryFrom<LogicImplInput<'a>> for LogicImpl<'a> {
    type Error = Errors<'a, ItemImpl>;

    // TODO: This unwrap() call needs to be corrected to return an error.
    #[expect(clippy::unwrap_in_result, reason = "TODO: This is temporary")]
    fn try_from(input: LogicImplInput<'a>) -> Result<Self, Self::Error> {
        let errors = Errors::new(input.item);

        for generic in &input.item.generics.params {
            if let GenericParam::Lifetime(params) = generic {
                if params.lifetime == *lifetimes::input() {
                    errors.subsume(SynError::new(
                        params.lifetime.span(),
                        ParseError::UseOfReservedLifetime,
                    ));
                }
                continue;
            }
            errors.subsume(SynError::new_spanned(
                generic,
                ParseError::NoGenericTypeSupport,
            ));
        }

        if input.item.trait_.is_some() {
            return Err(errors.finish(SynError::new_spanned(
                input.item,
                ParseError::NoTraitSupport,
            )));
        }

        let Some(type_) = typed_path(input.item.self_ty.as_ref(), false) else {
            return Err(errors.finish(SynError::new_spanned(
                &input.item.self_ty,
                ParseError::UnsupportedImplType,
            )));
        };

        let mut sanitizer = parse2::<Sanitizer<'_>>(type_.to_token_stream()).unwrap();

        let reserved_ident = idents::input();
        let reserved_lifetime = lifetimes::input();

        let cases = [
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
                Action::Forbid(ParseError::NoGenericLifetimeSupport),
            ),
        ];

        let outcome = sanitizer.sanitize(&cases);

        if let Err(err) = outcome.check() {
            errors.subsume(err);
        }

        if outcome.count(&Case::Ident(Some(&reserved_ident))) > 0 {
            // fail-fast due to reuse of the self ident for code generation
            return Err(errors);
        }

        let type_ = infallible!({ parse2(sanitizer.to_token_stream()) });

        let mut methods = vec![];
        let mut callbacks = vec![];

        for item in &input.item.items {
            if let ImplItem::Fn(method) = item {
                match LogicMethod::try_from(LogicMethodImplInput {
                    type_: &type_,
                    item: method,
                }) {
                    Ok(LogicMethod::Public(method)) => methods.push(method),
                    Ok(LogicMethod::Callback(callback)) => callbacks.push(callback),
                    Ok(LogicMethod::Private) => {}
                    Err(err) => errors.combine(&err),
                }
            }
        }

        errors.check()?;

        Ok(Self {
            type_,
            methods,
            callbacks,
            orig: input.item,
        })
    }
}
