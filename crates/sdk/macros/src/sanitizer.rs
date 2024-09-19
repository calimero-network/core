#![allow(dead_code, reason = "Will be used in future")]

#[cfg(test)]
#[path = "tests/sanitizer.rs"]
mod tests;

use core::cell::UnsafeCell;
use core::iter::once;
use std::collections::BTreeMap;

use proc_macro2::{Delimiter, Group, Punct, Spacing, Span, TokenStream, TokenTree};
use quote::{quote_spanned, ToTokens};
use syn::parse::{Parse, ParseStream};
use syn::{parse2, Error as SynError, Ident, Lifetime, Result as SynResult, Token};

use crate::errors::{Errors, ParseError};
use crate::macros::infallible;

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
    const fn as_ref(&'a self) -> &'a T {
        match self {
            MaybeOwned::Borrowed(t) => t,
            MaybeOwned::Owned(t) => t,
        }
    }
}

#[derive(Debug)]
enum SanitizerAtom<'a> {
    Self_(Token![Self]),
    Ident(Ident),
    Lifetime(LifetimeAtom),
    Tree(TokenTree),
    Stream(TokenStream),
    Group {
        entry: Sanitizer<'a>,
        delimiter: Delimiter,
        span: Span,
    },
}

#[derive(Debug)]
enum LifetimeAtom {
    Elided(Span),
    Named(Lifetime),
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Case<'a> {
    Self_,
    Ident(Option<&'a Ident>),
    Lifetime(Option<&'a Lifetime>),
}

pub struct Func<'a> {
    inner: UnsafeCell<&'a mut dyn FnMut(Span) -> Action<'a>>,
}

impl<'a> Func<'a> {
    pub fn new<F>(f: &'a mut F) -> Self
    where
        F: FnMut(Span) -> Action<'a> + 'a,
    {
        Self {
            inner: UnsafeCell::new(f),
        }
    }
}

pub enum Action<'a> {
    ReplaceWith(&'a dyn ToTokens),
    Forbid(ParseError<'static>),
    Custom(Func<'a>),
    Ignore,
}

impl<'a> SanitizerAtom<'a> {
    fn satisfies(&self, case: &Case<'a>) -> Option<Span> {
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
            (SanitizerAtom::Tree(TokenTree::Punct(punct)), Case::Lifetime(None)) => {
                (punct.as_char() == '&' && punct.spacing() == Spacing::Joint).then(|| punct.span())
            }
            _ => None,
        }
    }

    fn replace_with(&mut self, span: Span, replacement: &dyn ToTokens) -> bool {
        if let SanitizerAtom::Tree(TokenTree::Punct(punct)) = self {
            if punct.as_char() == '&' && punct.spacing() == Spacing::Joint {
                *punct = Punct::new('&', Spacing::Alone);
                punct.set_span(span);
                return false;
            }
        }

        *self = SanitizerAtom::Stream(quote_spanned!(span=> #replacement));
        true
    }

    fn apply_action(
        &mut self,
        span: Span,
        action: &Action<'_>,
        errors: &mut Errors<'static>,
    ) -> bool {
        match action {
            Action::ReplaceWith(replacement) => return self.replace_with(span, replacement),
            Action::Forbid(error) => errors.subsume(SynError::new(span, error)),
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
    errors: Errors<'static>,
}

impl<'a> SanitizationResult<'a> {
    /// panics: if this has been called before
    pub fn check(&self) -> Result<(), SynError> {
        if let Some(errors) = self.errors.take() {
            return Err(errors);
        }

        Ok(())
    }

    pub fn errors(&mut self) -> &mut Errors<'static> {
        &mut self.errors
    }

    pub fn len(&self) -> usize {
        self.counts.len()
    }

    pub fn count(&self, case: &'a Case<'a>) -> usize {
        self.counts.get(case).copied().unwrap_or_default()
    }
}

impl Sanitizer<'_> {
    pub fn sanitize<'a>(&mut self, cases: &'a [(Case<'a>, Action<'a>)]) -> SanitizationResult<'a> {
        let mut errors = Errors::default();

        let mut counts = BTreeMap::new();

        let MaybeOwned::Owned(entries) = &mut self.entries else {
            unreachable!("borrowed sanitizer leaked")
        };

        for entry in entries.iter_mut() {
            #[expect(clippy::wildcard_enum_match_arm, reason = "This is reasonable here")]
            match entry {
                SanitizerAtom::Group { entry, .. } => {
                    let SanitizationResult {
                        counts: sub_counts,
                        errors: err,
                    } = entry.sanitize(cases);

                    for (case, count) in sub_counts {
                        let _ = counts
                            .entry(case)
                            .and_modify(|e: &mut usize| *e = e.saturating_add(count))
                            .or_insert(count);
                    }

                    errors.combine(&err);
                }
                entry => {
                    for (case, action) in cases {
                        if let Some(span) = entry.satisfies(case) {
                            if entry.apply_action(span, action, &mut errors) {
                                let _ = counts
                                    .entry(case)
                                    .and_modify(|e| *e = e.saturating_add(1))
                                    .or_insert(1);
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

impl ToTokens for Sanitizer<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entries = self.entries.as_ref();

        for entry in entries {
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
                    let mut group = Group::new(*delimiter, entry.to_token_stream());
                    group.set_span(*span);
                    tokens.extend(once(TokenTree::Group(group)));
                }
            }
        }
    }
}

impl Parse for Sanitizer<'_> {
    fn parse(input: ParseStream<'_>) -> SynResult<Self> {
        let sanitizer = infallible!({
            let mut entries = Vec::new();

            while !input.is_empty() {
                if input.peek(Token![Self]) {
                    entries.push(SanitizerAtom::Self_(input.parse()?));
                } else if input.peek(Ident) {
                    entries.push(SanitizerAtom::Ident(input.parse()?));
                } else if input.peek(Lifetime) {
                    entries.push(SanitizerAtom::Lifetime(LifetimeAtom::Named(input.parse()?)));
                } else if input.peek(Token![&]) {
                    let and = input.parse::<TokenTree>()?;
                    let and_span = and.span();
                    entries.push(SanitizerAtom::Tree(and));
                    if input.peek(Lifetime) {
                        entries.push(SanitizerAtom::Lifetime(LifetimeAtom::Named(input.parse()?)));
                    } else {
                        entries.push(SanitizerAtom::Lifetime(LifetimeAtom::Elided(and_span)));
                    }
                } else {
                    #[expect(clippy::wildcard_enum_match_arm, reason = "This is reasonable here")]
                    match input.parse::<TokenTree>()? {
                        TokenTree::Group(group) => {
                            let entry = parse2(group.stream())?;
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

            SynResult::Ok(Sanitizer {
                entries: MaybeOwned::Owned(entries.into_boxed_slice()),
            })
        });

        Ok(sanitizer)
    }
}
