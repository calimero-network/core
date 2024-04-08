use proc_macro2::TokenTree;
use quote::{quote_spanned, ToTokens};
use syn::parse::Parse;

#[derive(Debug)]
pub struct Sanitizer<'a> {
    self_: Option<&'a syn::Path>,
    lifetime: Option<MaybeOwned<'a, String>>,
    entries: MaybeOwned<'a, Box<[SanitizerAtom<'a>]>>,
    metrics: MaybeOwned<'a, Metrics>,
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

type SelfType = syn::Token![Self];

#[derive(Debug)]
enum SanitizerAtom<'a> {
    Self_(SelfType),
    Lifetime(LifetimeAtom),
    Verbatim(proc_macro2::TokenTree),
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

#[derive(Clone, Debug, Default)]
pub struct Metrics {
    pub selves: usize,
    pub lifetimes: usize,
}

impl<'a> Sanitizer<'a> {
    pub fn with_self(mut self, self_: &'a syn::Path) -> Self {
        self.self_ = Some(self_);
        self
    }

    pub fn with_lifetime(mut self, lifetime: &'a syn::Ident) -> Self {
        self.lifetime = Some(MaybeOwned::Owned(format!("'{}", lifetime)));
        self
    }

    pub fn metrics(&self) -> &Metrics {
        self.metrics.as_ref()
    }
}

impl<'a> ToTokens for Sanitizer<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let entries = self.entries.as_ref();

        for entry in entries.iter() {
            match entry {
                SanitizerAtom::Self_(self_) => match &self.self_ {
                    Some(replacement) => {
                        quote_spanned!(self_.span=> #replacement).to_tokens(tokens)
                    }
                    None => self_.to_tokens(tokens),
                },
                SanitizerAtom::Lifetime(lifetime) => match &self.lifetime {
                    Some(ident) => syn::Lifetime::new(
                        ident.as_ref(),
                        match lifetime {
                            LifetimeAtom::Elided(span) => *span,
                            LifetimeAtom::Named(lifetime) => lifetime.span(),
                        },
                    )
                    .to_tokens(tokens),
                    None => match lifetime {
                        LifetimeAtom::Elided(_) => {}
                        LifetimeAtom::Named(lifetime) => lifetime.to_tokens(tokens),
                    },
                },
                SanitizerAtom::Verbatim(tt) => match (tt, &self.lifetime) {
                    (TokenTree::Punct(punct), Some(_))
                        if punct.as_char() == '&'
                            && punct.spacing() == proc_macro2::Spacing::Joint =>
                    {
                        let mut punct = proc_macro2::Punct::new('&', proc_macro2::Spacing::Alone);
                        punct.set_span(punct.span());
                        punct.to_tokens(tokens)
                    }
                    (tt, _) => tt.to_tokens(tokens),
                },
                SanitizerAtom::Group {
                    entry,
                    delimiter,
                    span,
                } => {
                    let entry = Sanitizer {
                        self_: self.self_,
                        lifetime: self
                            .lifetime
                            .as_ref()
                            .map(|lifetime| MaybeOwned::Borrowed(lifetime.as_ref())),
                        entries: MaybeOwned::Borrowed(entry.entries.as_ref()),
                        metrics: MaybeOwned::Borrowed(entry.metrics.as_ref()),
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
        let mut metrics = Metrics::default();

        while !input.is_empty() {
            if input.peek(syn::Token![Self]) {
                entries.push(SanitizerAtom::Self_(input.parse()?));
                metrics.selves += 1;
            } else if input.peek(syn::Lifetime) {
                entries.push(SanitizerAtom::Lifetime(LifetimeAtom::Named(input.parse()?)));
                metrics.lifetimes += 1;
            } else if input.peek(syn::Token![&]) {
                let and = input.parse::<proc_macro2::TokenTree>()?;
                let and_span = and.span();
                entries.push(SanitizerAtom::Verbatim(and));
                if input.peek(syn::Lifetime) {
                    entries.push(SanitizerAtom::Lifetime(LifetimeAtom::Named(input.parse()?)));
                } else {
                    entries.push(SanitizerAtom::Lifetime(LifetimeAtom::Elided(and_span)));
                }
                metrics.lifetimes += 1;
            } else {
                match input.parse::<TokenTree>()? {
                    TokenTree::Group(group) => {
                        entries.push(SanitizerAtom::Group {
                            entry: syn::parse2(group.stream())?,
                            delimiter: group.delimiter(),
                            span: group.span(),
                        });
                    }
                    tt => entries.push(SanitizerAtom::Verbatim(tt)),
                };
            }
        }

        Ok(Sanitizer {
            entries: MaybeOwned::Owned(entries.into_boxed_slice()),
            self_: None,
            lifetime: None,
            metrics: MaybeOwned::Owned(metrics),
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
        let replace_with: syn::Path = parse_quote! { crate::MyCustomType<'a> };

        let sanitized = syn::parse2::<Sanitizer>(ty)
            .unwrap()
            .with_self(&replace_with);

        let expected = quote! { crate::MyCustomType<'a> };

        assert_eq!(
            sanitized.to_token_stream().to_string(),
            expected.to_string()
        );

        let metrics = sanitized.metrics();

        assert_eq!(metrics.selves, 1);
        assert_eq!(metrics.lifetimes, 0);
    }

    #[test]
    fn test_self_sanitizer_complex() {
        let ty = quote! { &Some<Really<[Complex, Deep, Self, Type], Of, &mut Self>> };
        let replace_with: syn::Path = parse_quote! { crate::MyCustomType<'a> };

        let sanitized = syn::parse2::<Sanitizer>(ty)
            .unwrap()
            .with_self(&replace_with);

        let expected = quote! { &Some<Really<[Complex, Deep, crate::MyCustomType<'a>, Type], Of, &mut crate::MyCustomType<'a> >> };

        assert_eq!(
            sanitized.to_token_stream().to_string(),
            expected.to_string()
        );

        let metrics = sanitized.metrics();

        assert_eq!(metrics.selves, 2);
        assert_eq!(metrics.lifetimes, 2);
    }

    #[test]
    fn test_self_sanitizer_noop() {
        let ty = quote! { &Some<Really<[Complex, Deep, Self, Type], Of, &mut Self>> };

        let sanitized = syn::parse2::<Sanitizer>(ty.clone()).unwrap();

        assert_eq!(sanitized.to_token_stream().to_string(), ty.to_string());

        let metrics = sanitized.metrics();

        assert_eq!(metrics.selves, 2);
        assert_eq!(metrics.lifetimes, 2);
    }

    #[test]
    fn test_lifetime_sanitizer_simple() {
        let ty = quote! { &'a Some<'a, Complex<&&&Deep, &Type>> };
        let replace_with: syn::Ident = syn::Ident::new("static", proc_macro2::Span::call_site());

        let sanitized = syn::parse2::<Sanitizer>(ty)
            .unwrap()
            .with_lifetime(&replace_with);

        let expected = quote! { &'static Some<'static, Complex<&'static &'static &'static Deep, &'static Type>> };

        assert_eq!(
            sanitized.to_token_stream().to_string(),
            expected.to_string()
        );

        let metrics = sanitized.metrics();

        assert_eq!(metrics.selves, 0);
        assert_eq!(metrics.lifetimes, 6);
    }

    #[test]
    fn test_lifetime_sanitizer_complex() {
        let ty = quote! { &'a Some<'a, Complex<&&&Deep, &Type, Box<dyn MyTrait<'a, Output = &str> + 'a>>> };
        let replace_with: syn::Ident = syn::Ident::new("static", proc_macro2::Span::call_site());

        let sanitized = syn::parse2::<Sanitizer>(ty)
            .unwrap()
            .with_lifetime(&replace_with);

        let expected = quote! {
        &'static Some<
            'static,
            Complex<
                &'static &'static &'static Deep,
                &'static Type,
                Box<dyn MyTrait<'static, Output = &'static str> + 'static>>> };

        assert_eq!(
            sanitized.to_token_stream().to_string(),
            expected.to_string()
        );

        let metrics = sanitized.metrics();

        assert_eq!(metrics.selves, 0);
        assert_eq!(metrics.lifetimes, 9);
    }

    #[test]
    fn test_lifetime_sanitizer_noop() {
        let ty = quote! { &'a Some<'a, Complex<&&&Deep, &Type>> };

        let sanitized = syn::parse2::<Sanitizer>(ty.clone()).unwrap();

        assert_eq!(sanitized.to_token_stream().to_string(), ty.to_string());

        let metrics = sanitized.metrics();

        assert_eq!(metrics.selves, 0);
        assert_eq!(metrics.lifetimes, 6);
    }
}
