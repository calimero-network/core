//! Doc text for the ABI, read from the `#[doc = "..."]` attributes `///` desugars to.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Expr, ExprLit, Lit, Meta};

/// An item's doc: one line per attribute, one leading space stripped, outer
/// blank lines trimmed. `None` when nothing but blank lines remain.
pub fn doc_text(attrs: &[Attribute]) -> Option<String> {
    join_trimmed(&doc_lines(attrs))
}

/// The generated `Option<String>` expression for a doc.
pub fn tokens(doc: Option<&str>) -> TokenStream {
    match doc {
        Some(doc) => quote! { ::core::option::Option::Some(#doc.to_owned()) },
        None => quote! { ::core::option::Option::None },
    }
}

fn doc_lines(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter_map(|attr| {
            let Meta::NameValue(meta) = &attr.meta else {
                return None;
            };
            if !meta.path.is_ident("doc") {
                return None;
            }
            let Expr::Lit(ExprLit {
                lit: Lit::Str(value),
                ..
            }) = &meta.value
            else {
                return None;
            };
            let value = value.value();
            Some(value.strip_prefix(' ').unwrap_or(&value).to_owned())
        })
        .collect()
}

fn join_trimmed(lines: &[String]) -> Option<String> {
    let start = lines.iter().position(|line| !line.trim().is_empty())?;
    let end = lines.iter().rposition(|line| !line.trim().is_empty())?;
    Some(lines[start..=end].join("\n"))
}

#[cfg(test)]
mod tests {
    use syn::{parse_quote, Attribute};

    use super::{doc_text, tokens};

    fn attrs(lines: &[&str]) -> Vec<Attribute> {
        lines
            .iter()
            .map(|line| parse_quote!(#[doc = #line]))
            .collect()
    }

    #[test]
    fn no_doc_is_none() {
        let plain: Vec<Attribute> = vec![parse_quote!(#[must_use]), parse_quote!(#[doc(hidden)])];
        assert_eq!(doc_text(&plain), None);
        assert_eq!(doc_text(&attrs(&["", " ", ""])), None);
    }

    #[test]
    fn strips_one_leading_space_joins_lines_and_trims_blank_edges() {
        assert_eq!(
            doc_text(&attrs(&["", " Summary.", "", "   indented", " last", ""])).as_deref(),
            Some("Summary.\n\n  indented\nlast")
        );
    }

    #[test]
    fn tokens_render_an_option() {
        assert_eq!(
            tokens(None).to_string(),
            quote::quote!(::core::option::Option::None).to_string()
        );
        assert_eq!(
            tokens(Some("x")).to_string(),
            quote::quote!(::core::option::Option::Some("x".to_owned())).to_string()
        );
    }
}
