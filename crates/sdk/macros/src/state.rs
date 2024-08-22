use quote::{quote, ToTokens};
use syn::parse::Parse;

use crate::macros::infallible;
use crate::{errors, items, reserved, sanitizer};

#[derive(Clone, Copy)]
pub struct StateImpl<'a> {
    ident: &'a syn::Ident,
    generics: &'a syn::Generics,
    emits: &'a Option<MaybeBoundEvent>,
    orig: &'a items::StructOrEnumItem,
}

impl ToTokens for StateImpl<'_> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let StateImpl {
            ident,
            generics,
            emits,
            orig,
        } = *self;

        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        let mut lifetime = quote! { 'a };
        let mut event = quote! { ::calimero_sdk::event::NoEvent };

        if let Some(emits) = emits {
            if let Some(lt) = &emits.lifetime {
                lifetime = quote! { #lt };
            }
            event = {
                let event = &emits.ty;
                quote! { #event }
            };
        }

        quote! {
            #orig

            impl #impl_generics ::calimero_sdk::state::AppState for #ident #ty_generics #where_clause {
                type Event<#lifetime> = #event;
            }
        }
        .to_tokens(tokens)
    }
}

struct MaybeBoundEvent {
    lifetime: Option<syn::Lifetime>,
    ty: syn::Type,
}

// todo! move all errors to ParseError

impl Parse for MaybeBoundEvent {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut lifetime = None;

        let mut errors = errors::Errors::default();

        'bounds: {
            if input.peek(syn::Token![for]) {
                let bounds = match input.parse::<syn::BoundLifetimes>() {
                    Ok(bounds) => bounds,
                    Err(err) => {
                        errors.subsume(err);
                        break 'bounds;
                    }
                };

                let mut fine = true;

                if input.is_empty() {
                    errors.subsume(syn::Error::new_spanned(
                        &bounds,
                        "expected an event type to immediately follow",
                    ));

                    fine = false;
                }

                if bounds.lifetimes.is_empty() {
                    errors.subsume(syn::Error::new_spanned(
                        bounds.gt_token,
                        "non-empty lifetime bounds expected",
                    ));

                    fine = false;
                }

                if !fine {
                    return Err(errors.take().expect("not fine, so we must have errors"));
                }

                for param in bounds.lifetimes {
                    if let syn::GenericParam::Lifetime(syn::LifetimeParam {
                        lifetime: lt, ..
                    }) = param
                    {
                        if lifetime.is_some() {
                            errors.subsume(syn::Error::new(
                                lt.span(),
                                "only one lifetime can be specified",
                            ));

                            continue;
                        }
                        lifetime = Some(lt);
                    }
                }
            }
        }

        let ty = match input.parse::<syn::Type>() {
            Ok(ty) => ty,
            Err(err) => return Err(errors.subsumed(err)),
        };

        let mut sanitizer = syn::parse2::<sanitizer::Sanitizer<'_>>(ty.to_token_stream()).unwrap();

        let mut cases = vec![];

        if let Some(lt) = &lifetime {
            cases.push((
                sanitizer::Case::Lifetime(Some(lt)),
                sanitizer::Action::Ignore,
            ));
        }

        let mut unexpected_lifetime = |span: proc_macro2::Span| {
            let lifetime = span
                .source_text()
                .unwrap_or_else(|| "'{lifetime}".to_owned());

            // todo! source text is unreliable
            let error = if matches!(lifetime.as_str(), "&" | "'_") {
                errors::ParseError::MustSpecifyLifetime
            } else {
                errors::ParseError::UseOfUndeclaredLifetime {
                    append: format!(
                        "\n\nuse the `for<{}> {}` directive to declare it",
                        lifetime,
                        errors::Pretty::Type(&ty)
                    ),
                }
            };

            sanitizer::Action::Forbid(error)
        };

        let static_lifetime = syn::Lifetime::new("'static", proc_macro2::Span::call_site());

        cases.extend([
            (
                sanitizer::Case::Lifetime(Some(&static_lifetime)),
                sanitizer::Action::Ignore,
            ),
            (
                sanitizer::Case::Lifetime(None),
                sanitizer::Action::Custom(sanitizer::Func::new(&mut unexpected_lifetime)),
            ),
        ]);

        let mut outcome = sanitizer.sanitize(&cases);

        if let Some(lifetime) = &lifetime {
            if 0 == outcome.count(&sanitizer::Case::Lifetime(Some(lifetime)))
                && !(lifetime == &static_lifetime
                    || matches!(lifetime.ident.to_string().as_str(), "_"))
            {
                outcome.errors().subsume(syn::Error::new(
                    lifetime.span(),
                    "unused lifetime specified",
                ));
            }
        }

        outcome.check()?;

        let ty = infallible!({ syn::parse2(sanitizer.into_token_stream()) });

        input
            .is_empty()
            .then(|| MaybeBoundEvent { lifetime, ty })
            .ok_or_else(|| input.error("unexpected token"))
    }
}

pub struct StateArgs {
    emits: Option<MaybeBoundEvent>,
}

impl Parse for StateArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut emits = None;

        if !input.is_empty() {
            if !input.peek(syn::Ident) {
                return Err(input.error("expected an identifier"));
            }

            let ident = input.parse::<syn::Ident>()?;

            if !input.peek(syn::Token![=]) {
                let span = if let Some((tt, _)) = input.cursor().token_tree() {
                    tt.span()
                } else {
                    ident.span()
                };
                return Err(syn::Error::new(
                    span,
                    format_args!("expected `=` after `{ident}`"),
                ));
            }

            let eq = input.parse::<syn::Token![=]>()?;

            match ident.to_string().as_str() {
                "emits" => {
                    if input.is_empty() {
                        return Err(syn::Error::new_spanned(
                            eq,
                            "expected an event type after `=`",
                        ));
                    }
                    emits = Some(input.parse::<MaybeBoundEvent>()?)
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        &ident,
                        format_args!("unexpected `{ident}`"),
                    ));
                }
            }

            if !input.is_empty() {
                return Err(input.error("unexpected token"));
            }
        }

        Ok(StateArgs { emits })
    }
}

pub struct StateImplInput<'a> {
    pub item: &'a items::StructOrEnumItem,
    pub args: &'a StateArgs,
}

impl<'a> TryFrom<StateImplInput<'a>> for StateImpl<'a> {
    type Error = errors::Errors<'a, items::StructOrEnumItem>;

    fn try_from(input: StateImplInput<'a>) -> Result<Self, Self::Error> {
        let mut errors = errors::Errors::new(input.item);

        let (ident, generics) = match input.item {
            items::StructOrEnumItem::Struct(item) => (&item.ident, &item.generics),
            items::StructOrEnumItem::Enum(item) => (&item.ident, &item.generics),
        };

        if ident == &*reserved::idents::input() {
            errors.subsume(syn::Error::new_spanned(
                ident,
                errors::ParseError::UseOfReservedIdent,
            ));
        }

        for generic in &generics.params {
            match generic {
                syn::GenericParam::Lifetime(params) => {
                    errors.subsume(syn::Error::new(
                        params.lifetime.span(),
                        errors::ParseError::NoGenericLifetimeSupport,
                    ));
                }
                syn::GenericParam::Type(params) => {
                    if params.ident == *reserved::idents::input() {
                        errors.subsume(syn::Error::new_spanned(
                            &params.ident,
                            errors::ParseError::UseOfReservedIdent,
                        ));
                    }
                }
                syn::GenericParam::Const(_) => {}
            }
        }

        errors.check()?;

        Ok(StateImpl {
            ident,
            generics,
            emits: &input.args.emits,
            orig: input.item,
        })
    }
}
