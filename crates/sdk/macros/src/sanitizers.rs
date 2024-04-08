use proc_macro2::TokenTree;
use quote::ToTokens;
use syn::parse::Parse;

// parse-sanitize, as opposed to walking the syn tree and sanitizing

type SelfType = syn::Token![Self];

#[derive(Debug)]
enum SelfSanitizerEntry<T> {
    Self_(SelfType),
    Verbatim(proc_macro2::TokenTree),
    Group {
        entry: SelfSanitizer<T>,
        delimiter: proc_macro2::Delimiter,
        span: proc_macro2::Span,
    },
}

#[derive(Debug)]
pub struct SelfSanitizer<T> {
    replace_with: Option<T>,
    entries: Vec<SelfSanitizerEntry<T>>,
}

impl<T> SelfSanitizer<T> {
    pub fn replace_with(mut self, replace_with: T) -> Self {
        self.replace_with = Some(replace_with);
        self
    }
}

impl<T: ToTokens> ToTokens for SelfSanitizer<T> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        for entry in &self.entries {
            match entry {
                SelfSanitizerEntry::Self_(self_) => match &self.replace_with {
                    Some(replace_with) => replace_with.to_tokens(tokens),
                    None => self_.to_tokens(tokens),
                },
                SelfSanitizerEntry::Verbatim(tt) => tt.to_tokens(tokens),
                SelfSanitizerEntry::Group {
                    delimiter,
                    entry,
                    span,
                } => {
                    let mut group = proc_macro2::Group::new(*delimiter, entry.to_token_stream());
                    group.set_span(*span);
                    tokens.extend(std::iter::once(TokenTree::Group(group)))
                }
            }
        }
    }
}

impl<T> Parse for SelfSanitizer<T> {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut entries = Vec::new();

        while !input.is_empty() {
            if input.peek(syn::Token![Self]) {
                entries.push(SelfSanitizerEntry::Self_(
                    input.parse::<syn::Token![Self]>()?,
                ));
            } else {
                match input.parse::<TokenTree>()? {
                    TokenTree::Group(group) => {
                        entries.push(SelfSanitizerEntry::Group {
                            entry: syn::parse2(group.stream())?,
                            delimiter: group.delimiter(),
                            span: group.span(),
                        });
                    }
                    tt => entries.push(SelfSanitizerEntry::Verbatim(tt)),
                }
            };
        }

        Ok(SelfSanitizer {
            entries,
            replace_with: None,
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

        let sanitized = syn::parse2::<SelfSanitizer<_>>(ty)
            .unwrap()
            .replace_with(replace_with);

        let expected = quote! { crate::MyCustomType<'a> };

        assert_eq!(
            sanitized.to_token_stream().to_string(),
            expected.to_string()
        );
    }
    // let input = quote! {
    //     Self
    // };

    // let parsed = syn::parse2::<SelfSanitizer>(input)?;

    // parsed.replace_with(replace_with)

    // dbg!();

    // // panic!();

    // let input = quote! {
    //     [Vec<Self, Result<Self, Option<Self>>>; 10]
    // };

    // dbg!(syn::parse2::<SelfSanitizer>(input));

    // let input = quote! {
    //     &Vec<impl MyTrait<Output = Self>, Result<Self, Option<Self>>>
    // };

    // dbg!(syn::parse2::<SelfSanitizer>(input));

    // panic!();

    // let expected = SelfSanitizer {
    //     entries: vec![SelfSanitizerEntry::Self_(
    //         syn::parse2(quote! { Self }).unwrap(),
    //     )],
    // };
    // assert_eq!(syn::parse2::<SelfSanitizer>(input), Ok(expected));

    // let input = quote! {
    //     Self,
    //     Self,
    //     Self,
    // };
    // let expected = SelfSanitizer {
    //     entries: vec![
    //         SelfSanitizerEntry::Self_(syn::parse2(quote! { Self }).unwrap()),
    //         SelfSanitizerEntry::Self_(syn::parse2(quote! { Self }).unwrap()),
    //         SelfSanitizerEntry::Self_(syn::parse2(quote! { Self }).unwrap()),
    //     ],
    // };
    // assert_eq!(syn::parse2::<SelfSanitizer>(input), Ok(expected));

    // let input = quote! {};
    // }
}
