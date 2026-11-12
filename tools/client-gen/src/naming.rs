//! Rust names for the document's names: types in `PascalCase`, fields,
//! arguments and methods in `snake_case`, keywords as raw identifiers.

/// Rust's keywords, strict and reserved: a field or argument named one is
/// written `r#name`; a method name that would be one keeps its group prefix.
const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
    "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl",
    "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof",
    "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

pub fn is_keyword(s: &str) -> bool {
    KEYWORDS.contains(&s)
}

/// The words of a name: split at `_`, `-`, other punctuation and case changes
/// (`clientDataJson` is `client data json`, `3+` is `3 plus`).
fn words(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '+' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.push("plus".to_owned());
            continue;
        }
        if !c.is_ascii_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let prev = i.checked_sub(1).map(|j| chars[j]);
        let next = chars.get(i + 1).copied();
        let boundary = c.is_ascii_uppercase()
            && prev.is_some_and(|p| {
                p.is_ascii_lowercase()
                    || p.is_ascii_digit()
                    || (p.is_ascii_uppercase() && next.is_some_and(|n| n.is_ascii_lowercase()))
            });
        if boundary && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(c.to_ascii_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `PascalCase`; a name starting with a digit gets an `N` (`3+` is `N3Plus`).
pub fn pascal(s: &str) -> String {
    let mut out: String = words(s)
        .iter()
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or_else(String::new, |f| {
                f.to_ascii_uppercase().to_string() + c.as_str()
            })
        })
        .collect();
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, 'N');
    }
    out
}

/// `snake_case`, without the raw-identifier prefix.
pub fn snake(s: &str) -> String {
    let out = words(s).join("_");
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        format!("n{out}")
    } else {
        out
    }
}

/// `snake_case` as an identifier (`r#type` for a keyword).
pub fn ident(s: &str) -> String {
    let name = snake(s);
    if is_keyword(&name) {
        format!("r#{name}")
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::{ident, pascal, snake};

    #[test]
    fn names_follow_rust_case() {
        assert_eq!(snake("clientDataJson"), "client_data_json");
        assert_eq!(snake("appHash"), "app_hash");
        assert_eq!(snake("ttlSecs"), "ttl_secs");
        assert_eq!(snake("HTTPStatus"), "http_status");
        assert_eq!(ident("type"), "r#type");
        assert_eq!(pascal("readOnly"), "ReadOnly");
        assert_eq!(pascal("app_attestation"), "AppAttestation");
        assert_eq!(pascal("3+"), "N3Plus");
        assert_eq!(pascal("application/wasm"), "ApplicationWasm");
    }
}
