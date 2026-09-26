//! Doc text for the ABI, read from the `#[doc = "..."]` attributes `///` desugars to.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Expr, ExprLit, Lit, Meta};

const ARGUMENTS_HEADING: &str = "# Arguments"; // the method section holding per-parameter docs
const ENTRY_PREFIXES: [&str; 2] = ["* `", "- `"]; // list bullet plus the name's opening backtick
const ENTRY_SEPARATOR: &str = " - "; // between the closing backtick and the entry text
const LIST_BULLETS: [&str; 2] = ["* ", "- "]; // a list item in `# Arguments` must be a well-formed entry

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

/// A method's doc with its `# Arguments` section split out per parameter.
pub struct MethodDocs {
    pub doc: Option<String>,
    pub params: Vec<(String, String)>,
    pub malformed: Vec<String>,
}

impl MethodDocs {
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(param, _)| param == name)
            .map(|(_, text)| text.as_str())
            .filter(|text| !text.is_empty())
    }
}

pub fn method_docs(attrs: &[Attribute]) -> MethodDocs {
    let mut kept = Vec::new();
    let mut params: Vec<(String, String)> = Vec::new();
    let mut malformed = Vec::new();
    let mut in_arguments = false;
    let mut continuing = false;

    for line in doc_lines(attrs) {
        if line == ARGUMENTS_HEADING {
            in_arguments = true;
            continuing = false;
            continue;
        }
        if in_arguments && line.starts_with("# ") {
            in_arguments = false;
        }
        if !in_arguments {
            kept.push(line);
            continue;
        }
        if let Some((name, text)) = argument_entry(&line) {
            params.push((name.to_owned(), text.to_owned()));
            continuing = true;
            continue;
        }
        if LIST_BULLETS.iter().any(|bullet| line.starts_with(bullet)) {
            malformed.push(line);
            continuing = false;
            continue;
        }
        let continuation =
            continuing && line.starts_with(char::is_whitespace) && !line.trim().is_empty();
        match params.last_mut() {
            Some((_, text)) if continuation => {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(line.trim());
            }
            _ => continuing = false,
        }
    }

    MethodDocs {
        doc: join_trimmed(&kept),
        params,
        malformed,
    }
}

fn argument_entry(line: &str) -> Option<(&str, &str)> {
    let rest = ENTRY_PREFIXES
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))?;
    let (name, text) = rest.split_once('`')?;
    Some((name, text.strip_prefix(ENTRY_SEPARATOR)?.trim()))
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

    use super::{doc_text, method_docs, tokens};

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

    #[test]
    fn arguments_move_onto_params_and_other_sections_stay() {
        let docs = method_docs(&attrs(&[
            " Apply a batch of block edits.",
            "",
            " # Arguments",
            " * `edits` - at most 512 per call;",
            "   `b: 0` breaks the block.",
            " - `now` - caller's unix seconds.",
            "",
            " # Errors",
            " Fails past 512 edits.",
            "",
            " # Examples",
            " ```json",
            " {\"now\":1}",
            " ```",
        ]));
        assert_eq!(
            docs.doc.as_deref(),
            Some(
                "Apply a batch of block edits.\n\n# Errors\nFails past 512 edits.\n\n\
                 # Examples\n```json\n{\"now\":1}\n```"
            )
        );
        assert_eq!(
            docs.param("edits"),
            Some("at most 512 per call; `b: 0` breaks the block.")
        );
        assert_eq!(docs.param("now"), Some("caller's unix seconds."));
        assert_eq!(docs.param("missing"), None);
    }

    #[test]
    fn a_blank_line_ends_an_entry() {
        let docs = method_docs(&attrs(&[
            " # Arguments",
            " * `a` - first.",
            "",
            "   not a continuation",
        ]));
        assert_eq!(docs.param("a"), Some("first."));
        assert_eq!(docs.doc, None);
    }

    #[test]
    fn arguments_last_leave_no_trailing_blank_and_arguments_only_leave_no_doc() {
        let docs = method_docs(&attrs(&[
            " Summary.",
            "",
            " # Arguments",
            " * `a` - first.",
        ]));
        assert_eq!(docs.doc.as_deref(), Some("Summary."));
        let only = method_docs(&attrs(&[" # Arguments", " * `a` - first."]));
        assert_eq!(only.doc, None);
    }

    #[test]
    fn an_empty_entry_is_no_doc_but_still_names_a_param() {
        let docs = method_docs(&attrs(&[" # Arguments", " * `a` - "]));
        assert_eq!(docs.param("a"), None);
        assert_eq!(docs.params.len(), 1);
    }

    #[test]
    fn the_heading_must_match_exactly() {
        let docs = method_docs(&attrs(&[" # Arguments:", " * `a` - first."]));
        assert!(docs.params.is_empty());
        assert_eq!(docs.doc.as_deref(), Some("# Arguments:\n* `a` - first."));
    }

    #[test]
    fn no_doc_means_no_method_doc_and_no_params() {
        let plain: Vec<Attribute> = vec![parse_quote!(#[must_use])];
        let docs = method_docs(&plain);
        assert_eq!(docs.doc, None);
        assert!(docs.params.is_empty());
    }

    #[test]
    fn a_bullet_that_is_not_an_entry_is_malformed() {
        let docs = method_docs(&attrs(&[
            " * `outside`: bullets outside the section are prose.",
            "",
            " # Arguments",
            " * `x`: text",
            " * x - text",
            " - `y` - fine,",
            "   * an indented bullet continues it.",
        ]));
        assert_eq!(docs.malformed, ["* `x`: text", "* x - text"]);
        assert_eq!(
            docs.param("y"),
            Some("fine, * an indented bullet continues it.")
        );
        assert_eq!(docs.params.len(), 1);
        assert_eq!(
            docs.doc.as_deref(),
            Some("* `outside`: bullets outside the section are prose.")
        );
    }
}
