use std::cell::{Cell, RefCell};

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};

use crate::frontend::FRONTEND_ROOT;
use crate::FrontendError;

/// `<script type>` essences a browser executes besides an empty one (`module` and
/// `importmap` included); any other type makes the element an inert data block.
const SCRIPT_TYPES: &[&str] = &[
    "module",
    "importmap",
    "application/ecmascript",
    "application/javascript",
    "application/x-ecmascript",
    "application/x-javascript",
    "text/ecmascript",
    "text/javascript",
    "text/javascript1.0",
    "text/javascript1.1",
    "text/javascript1.2",
    "text/javascript1.3",
    "text/javascript1.4",
    "text/javascript1.5",
    "text/jscript",
    "text/livescript",
    "text/x-ecmascript",
    "text/x-javascript",
];

/// Elements whose content a browser reads as text, not markup, and how.
const RAW_TEXT: &[(&str, RawKind)] = &[
    ("script", RawKind::ScriptData),
    ("style", RawKind::Rawtext),
    ("xmp", RawKind::Rawtext),
    ("iframe", RawKind::Rawtext),
    ("noembed", RawKind::Rawtext),
    ("noframes", RawKind::Rawtext),
    ("noscript", RawKind::Rawtext), // the served page runs scripts
    ("textarea", RawKind::Rcdata),
    ("title", RawKind::Rcdata),
];

/// After one of these, markup is never skipped as text: foreign content and
/// `<select>` change how the tree builder treats the elements in RAW_TEXT.
const MARKUP_ONLY_AFTER: &[&str] = &["svg", "math", "select"];

/// Build-time lint for cargo mero on trusted build output; the node never calls it.
/// Not a security boundary (the origin and CSP are), and quadratic in one tag's attributes.
pub fn check_frontend_html(path: &str, bytes: &[u8]) -> Result<(), FrontendError> {
    let html =
        std::str::from_utf8(bytes).map_err(|_| FrontendError::InvalidHtml(path.to_owned()))?;
    let depth = path
        .strip_prefix(FRONTEND_ROOT)
        .unwrap_or(path)
        .matches('/')
        .count();
    let input = BufferQueue::default();
    input.push_back(StrTendril::from(html));
    // Only the tokenizer runs, since a tree build is quadratic on deep nesting.
    let sink = HtmlCheck {
        path,
        depth,
        ..HtmlCheck::default()
    };
    let tokenizer = Tokenizer::new(sink, TokenizerOpts::default());
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    tokenizer.sink.error.take().map_or(Ok(()), Err)
}

#[derive(Default)]
struct HtmlCheck<'a> {
    path: &'a str,
    depth: usize,
    markup_only: Cell<bool>,
    raw_open: Cell<Option<&'static str>>, // raw-text element a browser may still be inside
    running: Cell<bool>,                  // inside a `<script>` whose text runs as code
    error: RefCell<Option<FrontendError>>,
}

impl<'a> HtmlCheck<'a> {
    fn fail(&self, err: FrontendError) {
        self.error.borrow_mut().get_or_insert(err);
    }

    fn inline_script(&self, what: &str) {
        self.fail(FrontendError::InlineScript {
            path: self.path.to_owned(),
            what: what.to_owned(),
        });
    }

    fn start_tag(&self, tag: &Tag) -> TokenSinkResult<()> {
        let name: &str = &tag.name;
        let attrs: Vec<(&str, &str)> = tag
            .attrs
            .iter()
            .map(|a| {
                let attr: &str = &a.name.local;
                (attr.strip_prefix("xlink:").unwrap_or(attr), &*a.value)
            })
            .collect();
        if let Some(what) = inline_code(&attrs) {
            self.inline_script(&what);
        } else if let Err(err) = check_resources(self.path, name, &attrs, self.depth) {
            self.fail(err);
        }
        if self.raw_open.get().is_some() {
            self.inline_script("markup inside a raw-text element");
        }
        if name == "script" {
            self.running
                .set(attr(&attrs, "src").is_none() && is_executable(attr(&attrs, "type")));
        }
        if MARKUP_ONLY_AFTER.contains(&name) {
            self.markup_only.set(true);
        }
        let raw = RAW_TEXT.iter().find(|(raw, _)| *raw == name);
        if self.markup_only.get() {
            self.raw_open.set(
                raw.map(|(raw, _)| *raw)
                    .or((name == "plaintext").then_some("plaintext")),
            );
            return TokenSinkResult::Continue;
        }
        match raw {
            Some((_, kind)) => TokenSinkResult::RawData(*kind),
            None if name == "plaintext" => TokenSinkResult::Plaintext,
            None => TokenSinkResult::Continue,
        }
    }
}

impl TokenSink for HtmlCheck<'_> {
    type Handle = ();

    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        match token {
            Token::TagToken(tag) if tag.kind == TagKind::StartTag => return self.start_tag(&tag),
            Token::TagToken(tag) => {
                match self.raw_open.get() {
                    Some(open) if open == &*tag.name && open != "plaintext" => {
                        self.raw_open.set(None);
                    }
                    Some(_) => self.inline_script("markup inside a raw-text element"),
                    None => {}
                }
                if &*tag.name == "script" {
                    self.running.set(false);
                }
            }
            Token::CommentToken(text)
                if self.raw_open.get().is_some() || text.starts_with("[CDATA[") =>
            {
                self.inline_script("a comment or CDATA section a browser may read as text");
            }
            Token::CharacterTokens(text) if self.running.get() && !text.trim().is_empty() => {
                self.inline_script("inline script");
            }
            Token::NullCharacterToken if self.running.get() => {
                self.inline_script("inline script");
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

fn attr<'a>(attrs: &[(&str, &'a str)], name: &str) -> Option<&'a str> {
    attrs.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

/// Refuses a URL the element loads that the CSP or the path layout would break.
/// Navigation (`<a>`, `<area>`) is left alone; `data:` and `blob:` load only as non-code.
fn check_resources(
    path: &str,
    name: &str,
    attrs: &[(&str, &str)],
    depth: usize,
) -> Result<(), FrontendError> {
    if name == "base" {
        return Err(FrontendError::Base(path.to_owned()));
    }
    if matches!(name, "a" | "area") {
        return Ok(());
    }
    let is_code = name == "script"
        || (name == "link"
            && attr(attrs, "rel").is_some_and(|rel| {
                rel.split_ascii_whitespace().any(|token| {
                    token.eq_ignore_ascii_case("modulepreload")
                        || token.eq_ignore_ascii_case("preload")
                })
            }));
    for (attr, value) in attrs {
        let urls: Vec<&str> = match *attr {
            "src" | "href" | "poster" => vec![value],
            "srcset" => value
                .split(',')
                .filter_map(|c| c.split_whitespace().next())
                .collect(),
            _ => continue,
        };
        for raw in urls {
            let url = normalize_url(raw);
            if !is_code && (has_scheme(&url, "data") || has_scheme(&url, "blob")) {
                continue;
            }
            let err = if is_remote_url(&url) {
                FrontendError::RemoteResource {
                    path: path.to_owned(),
                    url: raw.to_owned(),
                }
            } else if escapes_bundle(&url, depth) {
                FrontendError::EscapingUrl {
                    path: path.to_owned(),
                    url: raw.to_owned(),
                }
            } else {
                continue;
            };
            return Err(err);
        }
    }
    Ok(())
}

/// What makes an element with these attributes run code the CSP forbids, if anything.
fn inline_code(attrs: &[(&str, &str)]) -> Option<String> {
    attrs.iter().find_map(|(name, value)| {
        let is_url = matches!(*name, "href" | "src" | "action" | "formaction");
        if name.starts_with("on") {
            Some(format!("event handler '{name}'"))
        } else if *name == "srcdoc" {
            Some("srcdoc".to_owned())
        } else if is_url && has_scheme(&normalize_url(value), "javascript") {
            Some(format!("{name}=\"{value}\""))
        } else {
            None
        }
    })
}

/// Whether a `<script>` with this `type` is run by a browser rather than an inert data block.
fn is_executable(script_type: Option<&str>) -> bool {
    script_type.is_none_or(|t| {
        let essence = t
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        essence.is_empty() || SCRIPT_TYPES.contains(&essence.as_str())
    })
}

/// The URL spec's preprocessing: trims spaces and drops tabs and newlines.
fn normalize_url(url: &str) -> String {
    url.trim_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect()
}

/// Whether a browser would resolve a normalized `url` against something other
/// than the document's own origin: any scheme, or a scheme-relative `//host`.
fn is_remote_url(url: &str) -> bool {
    let mut chars = url.chars();
    let first_two = (chars.next(), chars.next());
    matches!(first_two, (Some('/' | '\\'), Some('/' | '\\'))) || scheme(url).is_some()
}

/// The scheme of a normalized `url`, if it has one.
fn scheme(url: &str) -> Option<&str> {
    let len = url
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
        .unwrap_or(url.len());
    (len > 0 && url.starts_with(|c: char| c.is_ascii_alphabetic()) && url[len..].starts_with(':'))
        .then(|| &url[..len])
}

fn has_scheme(url: &str, expected: &str) -> bool {
    scheme(url).is_some_and(|s| s.eq_ignore_ascii_case(expected))
}

/// Whether a normalized, same-origin `url` is root-absolute or climbs above the
/// frontend root from a file `depth` folders below it.
fn escapes_bundle(url: &str, depth: usize) -> bool {
    if url.starts_with(['/', '\\']) {
        return true;
    }
    let path = url.split(['?', '#']).next().unwrap_or("");
    let mut depth = depth as isize;
    for segment in path.split(['/', '\\']) {
        match segment.to_ascii_lowercase().replace("%2e", ".").as_str() {
            "" | "." => {}
            ".." => depth -= 1,
            _ => depth += 1,
        }
        if depth < 0 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html_check(html: &str) -> Result<(), FrontendError> {
        check_frontend_html("frontend/index.html", html.as_bytes())
    }

    #[test]
    fn refuses_html_that_loads_code_from_outside_the_bundle() {
        for tag in [
            r#"<script src="https://cdn.example/x.js"></script>"#,
            r#"<SCRIPT SRC="//cdn.example/x.js"></SCRIPT>"#,
            r#"<script src="\\cdn.example/x.js"></script>"#,
            r#"<script src="  http://cdn.example/x.js"></script>"#,
            "<script src=\"ht\ttps://cdn.example/x.js\"></script>",
            r#"<script src="h&#116;tps://cdn.example/x.js"></script>"#,
            r#"<script src="data:text/javascript,alert(1)"></script>"#,
            r#"<link rel="modulepreload" href="data:text/javascript,1">"#,
            r#"<link rel="preload" as="script" href="data:text/javascript,1">"#,
            r#"<link rel="modulepreload" href="https://cdn.example/x.js">"#,
            r#"<link rel="preload stylesheet" as="script" href="https://cdn.example/x.js">"#,
            r#"<link rel="stylesheet" href="https://fonts.googleapis.com/css">"#,
            r#"<img src="https://cdn.example/a.png">"#,
            r#"<img srcset="a.png 1x, https://cdn.example/b.png 2x">"#,
            r#"<video poster="//cdn.example/p.png"></video>"#,
            r#"<iframe src="http://cdn.example/"></iframe>"#,
            r#"<svg><script href="https://cdn.example/x.js"></script></svg>"#,
            r#"<svg><script xlink:href="https://cdn.example/x.js"></script></svg>"#,
        ] {
            let err = html_check(&format!("<!doctype html><head>{tag}</head>")).unwrap_err();
            assert_eq!(err.code(), "FRONTEND_REMOTE_RESOURCE", "{tag}");
        }
    }

    #[test]
    fn refuses_urls_that_leave_the_bundle_folder() {
        for url in [
            "/assets/app.js",
            " /assets/app.js",
            "\\assets/app.js",
            "../app.js",
            "./../app.js",
            "a/../../app.js",
            "%2e%2e/app.js",
            "a/%2E%2e/../app.js",
            "a\\..\\..\\x.js",
        ] {
            let tag = format!(r#"<script src="{url}"></script>"#);
            let err = html_check(&tag).unwrap_err();
            assert_eq!(err.code(), "FRONTEND_ESCAPING_URL", "{url}");
        }
        for tag in [
            r#"<link rel="modulepreload" href="/c.js">"#,
            r#"<link rel="icon" href="/vite.svg">"#,
            r#"<img src="/a.png">"#,
            r#"<img srcset="a.png 1x, /b.png 2x">"#,
            r#"<video poster="../../p.png"></video>"#,
        ] {
            let err = html_check(tag).unwrap_err();
            assert_eq!(err.code(), "FRONTEND_ESCAPING_URL", "{tag}");
        }
    }

    #[test]
    fn refuses_any_base_element() {
        for tag in [
            r#"<base href="./">"#,
            r#"<base href="https://cdn.example/">"#,
        ] {
            let err = html_check(tag).unwrap_err();
            assert_eq!(err.code(), "FRONTEND_ESCAPING_URL", "{tag}");
            assert!(err.to_string().contains("<base>"), "{err}");
        }
    }

    #[test]
    fn accepts_deeply_nested_html_quickly() {
        let start = std::time::Instant::now();
        html_check(&"<span>".repeat(100_000)).unwrap();
        assert!(start.elapsed().as_secs() < 5, "{:?}", start.elapsed());
    }

    #[test]
    fn raw_text_elements_hide_their_content_from_the_scan() {
        html_check(
            r#"<title><img src="https://x/a.png"></title>
               <style><img src="https://x/a.png"></style>
               <textarea><img src="https://x/a.png"></textarea>
               <script type="text/template"><script src="https://x/a.js"></script>"#,
        )
        .unwrap();
    }

    #[test]
    fn foreign_content_and_select_are_scanned_as_markup() {
        let html = r#"<svg><script href="a.js"><img src="https://x/a.png"></script></svg>"#;
        assert_eq!(
            html_check(html).unwrap_err().code(),
            "FRONTEND_REMOTE_RESOURCE"
        );
        let err = html_check("<svg><script><b></b>x()</script></svg>").unwrap_err();
        assert_eq!(err.code(), "FRONTEND_INLINE_SCRIPT");
        html_check("<svg><style>.a{fill:red}</style><title>t</title></svg><p>text</p>").unwrap();
    }

    #[test]
    fn markup_inside_a_raw_text_element_is_refused_once_readings_can_differ() {
        for html in [
            r#"<svg></svg><style><!--</style><script src="/other-app/x.js"></script>-->"#,
            r#"<select><textarea><!--</textarea><script src="../../x.js"></script>--></textarea>"#,
            r#"<math><mtext><title><x a="</title><script src=../../x.js></script>"></title>"#,
            "<select><xmp></select><img src=https://x/a.png></xmp>",
            "<svg></svg><plaintext><b>",
        ] {
            let err = html_check(html).unwrap_err();
            assert_eq!(err.code(), "FRONTEND_INLINE_SCRIPT", "{html}");
        }
    }

    #[test]
    fn plaintext_hides_the_rest_of_the_file() {
        html_check(r#"<plaintext><script src="https://x/a.js"></script>"#).unwrap();
    }

    #[test]
    fn tokenizer_edge_cases_still_count_as_inline_code() {
        for html in [
            "<svg></svg><plaintext></plaintext>",
            "<select><script>\0</script>",
            r#"<svg><![CDATA[ > <!-- ]]><script href="/elsewhere/x.js"/> --></svg>"#,
            "<select><script/>alert(1)</script>",
            "<svg></svg><script/>x()</script>",
            "<script>alert(1)",
        ] {
            let err = html_check(html).unwrap_err();
            assert_eq!(err.code(), "FRONTEND_INLINE_SCRIPT", "{html}");
        }
    }

    #[test]
    fn climbing_is_measured_from_the_html_files_own_folder() {
        let html = r#"<script src="../app.js"></script>"#;
        check_frontend_html("frontend/sub/index.html", html.as_bytes()).unwrap();
        let html = r#"<script src="../../app.js"></script>"#;
        let err = check_frontend_html("frontend/sub/index.html", html.as_bytes()).unwrap_err();
        assert_eq!(err.code(), "FRONTEND_ESCAPING_URL");
    }

    #[test]
    fn refuses_html_that_runs_inline_code() {
        for html in [
            "<script>alert(1)</script>",
            r#"<script type="module">import 'x'</script>"#,
            r#"<script type="">x()</script>"#,
            r#"<script type=" Text/JavaScript; charset=utf-8">x()</script>"#,
            r#"<script type="importmap">{}</script>"#,
            r#"<body onload="x()">"#,
            r#"<img src="a.png" ONERROR="x()">"#,
            r#"<a href="javascript:x()">a</a>"#,
            "<a href=\" jav\tascript:x()\">a</a>",
            r#"<form action="JavaScript:x()"></form>"#,
            r#"<button formaction="javascript:x()"></button>"#,
            r#"<iframe src="javascript:x()"></iframe>"#,
            r#"<iframe srcdoc="<script>x()</script>"></iframe>"#,
        ] {
            let err = html_check(&format!("<!doctype html>{html}")).unwrap_err();
            assert_eq!(err.code(), "FRONTEND_INLINE_SCRIPT", "{html}");
        }
    }

    #[test]
    fn allows_data_blocks_and_empty_script_elements() {
        html_check(
            r##"<script type="application/json">{"a":1}</script>
               <script type="text/template">x</script>
               <script src="a.js">x</script>
               <script src="a.js">  </script>
               <img src="data:image/png;base64,AAAA" srcset="a.png 1x, b.png 2x">
               <img src="blob:null/1">
               <link rel="stylesheet" href="style.css">
               <style>body { color: red }</style>
               <p style="color: red">x</p>
               <script>   </script>
               <a href="#top" title="onclick">x</a>"##,
        )
        .unwrap();
    }

    #[test]
    fn refuses_html_that_is_not_utf8() {
        let bytes = b"<script src=\"/a.js\"></script>\xff";
        let err = check_frontend_html("frontend/index.html", bytes).unwrap_err();
        assert_eq!(err.code(), "FRONTEND_INVALID_HTML");
    }

    #[test]
    fn accepts_html_that_loads_code_from_the_bundle() {
        html_check(
            r#"<!doctype html><head>
                <script type="module" src="./assets/app.js"></script>
                <script src="./vendor.js"></script>
                <script src="app.js?p=/../../x"></script>
                <script src="app.js#/../../x"></script>
                <link rel="modulepreload" href="assets/chunk.js">
                <a href="https://docs.example/">docs</a>
                <a href="/elsewhere">same-origin link</a>
            </head>"#,
        )
        .unwrap();
    }
}
