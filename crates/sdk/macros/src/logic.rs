use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{parse2, Attribute, Error as SynError, GenericParam, ImplItem, ItemImpl, Path};

use crate::errors::{Errors, ParseError};
use crate::logic::method::{LogicMethod, LogicMethodImplInput, PublicLogicMethod};
use crate::logic::utils::typed_path;
use crate::macros::infallible;
use crate::reserved::{idents, lifetimes};
use crate::sanitizer::{Action, Case, Sanitizer};

mod arg;
mod method;
mod ty;
mod utils;

/// Whether any attribute in the list is `#[app::init]`. Mirrors the per-method
/// detection in `logic/method.rs`, but is applied at the impl level so the
/// duplicate-init check doesn't depend on the method parsing successfully.
fn has_init_attr(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let segments = &attr.path().segments;
        segments.len() == 2 && segments[0].ident == "app" && segments[1].ident == "init"
    })
}

pub struct LogicImpl<'a> {
    #[expect(dead_code, reason = "This will be used in future")]
    type_: Path,
    methods: Vec<PublicLogicMethod<'a>>,
    orig: &'a ItemImpl,
}

impl ToTokens for LogicImpl<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let LogicImpl { orig, methods, .. } = self;

        quote! {
            #orig

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

        for item in &input.item.items {
            if let ImplItem::Fn(method) = item {
                match LogicMethod::try_from(LogicMethodImplInput {
                    type_: &type_,
                    item: method,
                }) {
                    Ok(LogicMethod::Public(method)) => methods.push(*method),
                    Ok(LogicMethod::Private) => {}
                    Err(err) => errors.combine(&err),
                }
            }
        }

        // At most one `#[app::init]` per type — multiple initializers are
        // ambiguous (which one constructs the state?). Count the attribute
        // directly on the impl items, independent of per-method parsing: an
        // initializer that fails its *own* validation (e.g. not named `init`) is
        // excluded from `methods`, so a `methods`-based check would miss it.
        // When there's more than one, flag *every* `#[app::init]` (not just the
        // ones after the first) so the error never lands on an arbitrary "valid"
        // initializer while the real offender — which may appear earlier in
        // source order — goes unmarked. These accumulate with per-method errors.
        let init_methods: Vec<_> = input
            .item
            .items
            .iter()
            .filter_map(|item| match item {
                ImplItem::Fn(method) if has_init_attr(&method.attrs) => Some(method),
                _ => None,
            })
            .collect();
        if init_methods.len() > 1 {
            for method in &init_methods {
                errors.subsume(SynError::new_spanned(
                    &method.sig.ident,
                    ParseError::DuplicateInit,
                ));
            }
        }

        errors.check()?;

        Ok(Self {
            type_,
            methods,
            orig: input.item,
        })
    }
}
