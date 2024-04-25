use std::cell::UnsafeCell;
use std::collections::BTreeMap;

use proc_macro2::TokenTree;
use quote::{quote_spanned, ToTokens};
use syn::parse::Parse;

use crate::errors;

#[derive(Debug)]
pub struct Sanitizer<'a> {
    entries: MaybeOwned<'a, Box<[SanitizerAtom<'a>]>>,
}

#[derive(Debug)]
enum MaybeOwned<'a, T> {
    Borrowed(&'a T),
    Owned(T),
}

// couldn't use Cow because `<Cow<'a, T> as AsRef>::as_ref` doesn't return `&'a T`
impl<'a, T> MaybeOwned<'a, T> {
    fn as_ref(&'a self) -> &'a T {
        match self {
            MaybeOwned::Borrowed(t) => t,
            MaybeOwned::Owned(t) => t,
        }
    }
}

#[derive(Debug)]
enum SanitizerAtom<'a> {
    Self_(syn::Token![Self]),
    Ident(syn::Ident),
    Lifetime(LifetimeAtom),
    Tree(proc_macro2::TokenTree),
    Stream(proc_macro2::TokenStream),
    Group {
        entry: Sanitizer<'a>,
        delimiter: proc_macro2::Delimiter,
        span: proc_macro2::Span,
    },
}

#[derive(Debug)]
enum LifetimeAtom {
    Elided(proc_macro2::Span),
    Named(syn::Lifetime),
}

#[derive(Eq, Ord, Debug, PartialEq, PartialOrd)]
pub enum Case<'a> {
    Self_,
    Ident(Option<&'a syn::Ident>),
    Lifetime(Option<&'a syn::Lifetime>),
}

pub struct Func<'a> {
    inner: UnsafeCell<&'a mut dyn FnMut(proc_macro2::Span) -> Action<'a>>,
}

impl<'a> Func<'a> {
    pub fn new<F>(f: &'a mut F) -> Self
    where
        F: FnMut(proc_macro2::Span) -> Action<'a> + 'a,
    {
        Self {
            inner: UnsafeCell::new(f as _),
        }
    }
}

pub enum Action<'a> {
    ReplaceWith(&'a dyn ToTokens),
    Forbid(errors::ParseError<'static>),
    Custom(Func<'a>),
    Ignore,
}

impl<'a> SanitizerAtom<'a> {
    fn satisfies(&self, case: &Case<'a>) -> Option<proc_macro2::Span> {
        match (self, case) {
            (SanitizerAtom::Self_(self_), Case::Self_) => Some(self_.span),
            (SanitizerAtom::Ident(ident), Case::Ident(Some(case_ident))) => {
                (ident == *case_ident).then(|| ident.span())
            }
            (SanitizerAtom::Ident(ident), Case::Ident(None)) => Some(ident.span()),
            (
                SanitizerAtom::Lifetime(LifetimeAtom::Named(lifetime)),
                Case::Lifetime(Some(case_lifetime)),
            ) => (lifetime == *case_lifetime).then(|| lifetime.span()),
            (SanitizerAtom::Lifetime(lifetime), Case::Lifetime(None)) => match lifetime {
                LifetimeAtom::Named(lifetime) => Some(lifetime.span()),
                LifetimeAtom::Elided(span) => Some(*span),
            },
            (SanitizerAtom::Tree(proc_macro2::TokenTree::Punct(punct)), Case::Lifetime(None)) => {
                (punct.as_char() == '&' && punct.spacing() == proc_macro2::Spacing::Joint)
                    .then(|| punct.span())
            }
            _ => None,
        }
    }

    fn replace_with(&mut self, span: proc_macro2::Span, replacement: &dyn ToTokens) -> bool {
        if let SanitizerAtom::Tree(proc_macro2::TokenTree::Punct(punct)) = self {
            if punct.as_char() == '&' && punct.spacing() == proc_macro2::Spacing::Joint {
                *punct = proc_macro2::Punct::new('&', proc_macro2::Spacing::Alone);
                punct.set_span(span);
                return false;
            }
        }

        *self = SanitizerAtom::Stream(quote_spanned!(span=> #replacement));
        true
    }

    fn apply_action<'b>(
        &mut self,
        span: proc_macro2::Span,
        action: &Action<'b>,
        errors: &mut errors::Errors<'static>,
    ) -> bool {
        match action {
            Action::ReplaceWith(replacement) => return self.replace_with(span, replacement),
            Action::Forbid(error) => errors.subsume(syn::Error::new(span, error)),
            Action::Custom(func) => {
                let func = unsafe { &mut *func.inner.get() };
                return self.apply_action(span, &func(span), errors);
            }
            Action::Ignore => {}
        }
        true
    }
}

#[derive(Debug)]
pub struct SanitizationResult<'a> {
    counts: BTreeMap<&'a Case<'a>, usize>,
    errors: errors::Errors<'static>,
}

impl<'a> SanitizationResult<'a> {
    /// panics: if this has been called before
    pub fn check(&self) -> Result<(), syn::Error> {
        if let Some(errors) = self.errors.take() {
            return Err(errors);
        }

        Ok(())
    }

    pub fn errors(&mut self) -> &mut errors::Errors<'static> {
        &mut self.errors
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.counts.len()
    }

    pub fn count(&self, case: &'a Case<'a>) -> usize {
        self.counts.get(case).copied().unwrap_or_default()
    }
}

impl Sanitizer<'_> {
    pub fn sanitize<'a>(&mut self, cases: &'a [(Case<'a>, Action<'a>)]) -> SanitizationResult<'a> {
        let mut errors = errors::Errors::default();

        let mut counts = BTreeMap::new();

        let MaybeOwned::Owned(entries) = &mut self.entries else {
            unreachable!("borrowed sanitizer leaked")
        };

        for entry in entries.iter_mut() {
            match entry {
                SanitizerAtom::Group { entry, .. } => {
                    let SanitizationResult {
                        counts: sub_counts,
                        errors: err,
                    } = entry.sanitize(cases);

                    for (case, count) in sub_counts {
                        *counts.entry(case).or_insert(0) += count;
                    }

                    errors.combine(err);
                }
                entry => {
                    for (case, action) in cases.iter() {
                        if let Some(span) = entry.satisfies(case) {
                            if entry.apply_action(span, action, &mut errors) {
                                *counts.entry(case).or_insert(0) += 1;
                            }
                            break;
                        }
                    }
                }
            }
        }

        SanitizationResult { counts, errors }
    }
}

impl<'a> ToTokens for Sanitizer<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let entries = self.entries.as_ref();

        for entry in entries.iter() {
            match entry {
                SanitizerAtom::Self_(self_) => self_.to_tokens(tokens),
                SanitizerAtom::Ident(ident) => ident.to_tokens(tokens),
                SanitizerAtom::Lifetime(lifetime) => match lifetime {
                    LifetimeAtom::Elided(_) => {}
                    LifetimeAtom::Named(lifetime) => lifetime.to_tokens(tokens),
                },
                SanitizerAtom::Tree(tt) => tt.to_tokens(tokens),
                SanitizerAtom::Stream(stream) => stream.to_tokens(tokens),
                SanitizerAtom::Group {
                    entry,
                    delimiter,
                    span,
                } => {
                    let entry = Sanitizer {
                        entries: MaybeOwned::Borrowed(entry.entries.as_ref()),
                    };
                    let mut group = proc_macro2::Group::new(*delimiter, entry.to_token_stream());
                    group.set_span(*span);
                    tokens.extend(std::iter::once(TokenTree::Group(group)))
                }
            }
        }
    }
}

impl<'a> Parse for Sanitizer<'a> {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut entries = Vec::new();

        while !input.is_empty() {
            if input.peek(syn::Token![Self]) {
                entries.push(SanitizerAtom::Self_(input.parse()?));
            } else if input.peek(syn::Ident) {
                entries.push(SanitizerAtom::Ident(input.parse()?));
            } else if input.peek(syn::Lifetime) {
                entries.push(SanitizerAtom::Lifetime(LifetimeAtom::Named(input.parse()?)));
            } else if input.peek(syn::Token![&]) {
                let and = input.parse::<proc_macro2::TokenTree>()?;
                let and_span = and.span();
                entries.push(SanitizerAtom::Tree(and));
                if input.peek(syn::Lifetime) {
                    entries.push(SanitizerAtom::Lifetime(LifetimeAtom::Named(input.parse()?)));
                } else {
                    entries.push(SanitizerAtom::Lifetime(LifetimeAtom::Elided(and_span)));
                }
            } else {
                match input.parse::<TokenTree>()? {
                    TokenTree::Group(group) => {
                        let entry = syn::parse2(group.stream())?;
                        entries.push(SanitizerAtom::Group {
                            entry,
                            delimiter: group.delimiter(),
                            span: group.span(),
                        });
                    }
                    tt => entries.push(SanitizerAtom::Tree(tt)),
                };
            }
        }

        Ok(Sanitizer {
            entries: MaybeOwned::Owned(entries.into_boxed_slice()),
        })
    }
}

#[cfg(test)]
mod tests {
    use quote::quote;
    use syn::parse_quote;

    use super::*;

    #[test]
    fn test_self_sanitizer_simple() {
        let ty = quote! { Self };
        let my_custom_type: syn::Path = parse_quote! { crate::MyCustomType<'a> };

        let mut sanitizer = syn::parse2::<Sanitizer>(ty).unwrap();

        let cases = [(Case::Self_, Action::ReplaceWith(&my_custom_type))];

        let outcome = sanitizer.sanitize(&cases);

        outcome.check().unwrap();

        assert_eq!(outcome.len(), 1);
        assert_eq!(outcome.count(&Case::Self_), 1);

        let expected = quote! { crate::MyCustomType<'a> };

        assert_eq!(
            sanitizer.to_token_stream().to_string(),
            expected.to_string()
        );
    }

    #[test]
    fn test_ident_sanitizer_complex() {
        let ty = quote! { &Some<Really<[Complex, Deep, Self, Type], Of, &mut Self>> };
        let my_custom_type: syn::Path = parse_quote! { crate::MyCustomType<'a> };
        let complex_type: syn::Ident = parse_quote! { Complex };
        let other_type: syn::Type = parse_quote! { Box<dyn MyTrait<OtherType>> };
        let really_type: syn::Ident = parse_quote! { Really };
        let deep_type: syn::Ident = parse_quote! { Deep };

        let mut sanitizer = syn::parse2::<Sanitizer>(ty).unwrap();

        let cases = [
            (Case::Self_, Action::ReplaceWith(&my_custom_type)),
            (
                Case::Ident(Some(&complex_type)),
                Action::ReplaceWith(&other_type),
            ),
            (
                Case::Ident(Some(&really_type)),
                Action::ReplaceWith(&complex_type),
            ),
            (Case::Ident(None), Action::ReplaceWith(&really_type)),
            (
                Case::Ident(Some(&deep_type)),
                Action::Forbid(errors::ParseError::UseOfReservedIdent),
            ),
        ];

        let outcome = sanitizer.sanitize(&cases);

        if let Err(err) = outcome.check() {
            let mut errs = err.into_iter();
            assert_eq!(
                errs.next().unwrap().to_string(),
                errors::ParseError::UseOfReservedIdent.to_string()
            );
        }

        assert_eq!(outcome.len(), 4);
        assert_eq!(outcome.count(&Case::Self_), 2);
        assert_eq!(outcome.count(&Case::Ident(Some(&complex_type))), 1);
        assert_eq!(outcome.count(&Case::Ident(None)), 4);
        assert_eq!(outcome.count(&Case::Ident(Some(&really_type))), 1);

        let expected = quote! {
            &Really<
                Complex<
                    [
                        Box<
                            dyn MyTrait<OtherType>
                        >,
                        Really,
                        crate::MyCustomType<'a>,
                        Really
                    ],
                    Really,
                    &mut crate::MyCustomType<'a>
                >>
        };

        assert_eq!(
            sanitizer.to_token_stream().to_string(),
            expected.to_string()
        );
    }

    #[test]
    fn test_self_sanitizer_noop() {
        let ty = quote! { &Some<Really<[Complex, Deep, Self, Type], Of, &mut Self>> };

        let mut sanitizer = syn::parse2::<Sanitizer>(ty.clone()).unwrap();

        let outcome = sanitizer.sanitize(&[]);

        outcome.check().unwrap();

        assert_eq!(outcome.len(), 0);

        assert_eq!(sanitizer.to_token_stream().to_string(), ty.to_string());
    }

    #[test]
    fn test_lifetime_sanitizer_simple() {
        let ty = quote! { &'a Some<'a, Complex<&&&Deep, &Type>> };
        let replace_with = syn::Lifetime::new("'static", proc_macro2::Span::call_site());

        let mut sanitizer = syn::parse2::<Sanitizer>(ty).unwrap();

        let cases = [(Case::Lifetime(None), Action::ReplaceWith(&replace_with))];

        let outcome = sanitizer.sanitize(&cases);

        outcome.check().unwrap();

        assert_eq!(outcome.len(), 1);
        assert_eq!(outcome.count(&Case::Lifetime(None)), 6);

        let expected = quote! { &'static Some<'static, Complex<&'static &'static &'static Deep, &'static Type>> };

        assert_eq!(
            sanitizer.to_token_stream().to_string(),
            expected.to_string()
        );
    }

    #[test]
    fn test_lifetime_sanitizer_specialized() {
        let ty = quote! { &'a Some<'a, Complex<&&&Deep, &'b Type>> };
        let a_lifetime = syn::Lifetime::new("'a", proc_macro2::Span::call_site());
        let b_lifetime = syn::Lifetime::new("'b", proc_macro2::Span::call_site());
        let static_lifetime = syn::Lifetime::new("'static", proc_macro2::Span::call_site());

        let mut sanitizer = syn::parse2::<Sanitizer>(ty).unwrap();

        let cases = [
            (
                Case::Lifetime(Some(&a_lifetime)),
                Action::ReplaceWith(&b_lifetime),
            ),
            (Case::Lifetime(None), Action::ReplaceWith(&static_lifetime)),
            (
                Case::Lifetime(Some(&b_lifetime)),
                Action::ReplaceWith(&a_lifetime),
            ),
        ];

        let outcome = sanitizer.sanitize(&cases);

        outcome.check().unwrap();

        assert_eq!(outcome.len(), 2);
        assert_eq!(outcome.count(&Case::Lifetime(None)), 4);
        assert_eq!(outcome.count(&Case::Lifetime(Some(&a_lifetime))), 2);
        assert_eq!(outcome.count(&Case::Lifetime(Some(&b_lifetime))), 0);

        let expected =
            quote! { &'b Some<'b, Complex<&'static &'static &'static Deep, &'static Type>> };

        assert_eq!(
            sanitizer.to_token_stream().to_string(),
            expected.to_string()
        );
    }

    #[test]
    fn test_lifetime_sanitizer_complex() {
        let ty = quote! { &'a Some<'a, Complex<&&&Deep, &Type, Box<dyn MyTrait<'b, Output = (&str, &'b str)> + 'b>>> };
        let a_lifetime = syn::Lifetime::new("'a", proc_macro2::Span::call_site());
        let b_lifetime = syn::Lifetime::new("'b", proc_macro2::Span::call_site());
        let static_lifetime = syn::Lifetime::new("'static", proc_macro2::Span::call_site());

        let mut sanitizer = syn::parse2::<Sanitizer>(ty).unwrap();

        let cases = [
            (
                Case::Lifetime(Some(&a_lifetime)),
                Action::ReplaceWith(&b_lifetime),
            ),
            (
                Case::Lifetime(Some(&b_lifetime)),
                Action::Forbid(errors::ParseError::UseOfReservedLifetime),
            ),
            (Case::Lifetime(None), Action::ReplaceWith(&static_lifetime)),
        ];

        let outcome = sanitizer.sanitize(&cases);

        if let Err(err) = outcome.check() {
            let mut errs = err.into_iter();
            assert_eq!(
                errs.next().unwrap().to_string(),
                errors::ParseError::UseOfReservedLifetime.to_string()
            );
        }

        assert_eq!(outcome.len(), 3);
        assert_eq!(outcome.count(&Case::Lifetime(Some(&a_lifetime))), 2);
        assert_eq!(outcome.count(&Case::Lifetime(Some(&b_lifetime))), 3);
        assert_eq!(outcome.count(&Case::Lifetime(None)), 5);

        let expected = quote! {
            &'b Some<
                'b,
                Complex<
                    &'static &'static &'static Deep,
                    &'static Type,
                    Box<dyn MyTrait<'b, Output = (&'static str, &'b str)> + 'b>>
                >
        };

        assert_eq!(
            sanitizer.to_token_stream().to_string(),
            expected.to_string()
        );
    }

    #[test]
    fn test_lifetime_sanitizer_noop() {
        let ty = quote! { &'a Some<'a, Complex<&&&Deep, &Type>> };

        let mut sanitizer = syn::parse2::<Sanitizer>(ty.clone()).unwrap();

        let outcome = sanitizer.sanitize(&[]);

        outcome.check().unwrap();

        assert_eq!(outcome.len(), 0);

        assert_eq!(sanitizer.to_token_stream().to_string(), ty.to_string());
    }
}
