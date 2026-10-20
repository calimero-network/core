use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::BundleArtifact;

const FRONTEND_ROOT: &str = "frontend/"; // every frontend file's archive path starts here
pub const MAX_FRONTEND_BYTES: u64 = 32 * 1024 * 1024; // sum of every frontend file's declared size
const HTML_MIME: &str = "text/html; charset=utf-8";
const PATH_PUNCTUATION: &str = "._~@+-"; // the only non-alphanumerics a path component may hold

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

/// The only file types a node serves, by lowercase extension. Anything else is
/// refused at bundling and install, so nothing is ever served by sniffing.
const MIME_TYPES: &[(&str, &str)] = &[
    ("html", HTML_MIME),
    ("js", "text/javascript; charset=utf-8"),
    ("mjs", "text/javascript; charset=utf-8"),
    ("css", "text/css; charset=utf-8"),
    ("json", "application/json"),
    ("webmanifest", "application/manifest+json"),
    ("txt", "text/plain; charset=utf-8"),
    ("svg", "image/svg+xml"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("ico", "image/x-icon"),
    ("woff", "font/woff"),
    ("woff2", "font/woff2"),
    ("ttf", "font/ttf"),
    ("wasm", "application/wasm"),
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

/// A static web build shipped inside the bundle and covered by its signature.
/// Every path is an archive path under `frontend/`; `entry` names one of `files`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleFrontend {
    pub entry: String,
    pub files: Vec<BundleArtifact>,
}

/// Why a bundled frontend is refused. `code()` is stable: `cargo mero` and the
/// node report the same one, so tooling matches on it rather than the message.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum FrontendError {
    #[error("[FRONTEND_UNSAFE_PATH] frontend file '{0}' is not a canonical path under '{FRONTEND_ROOT}'")]
    UnsafePath(String),
    #[error("[FRONTEND_DUPLICATE_PATH] frontend file '{0}' is listed more than once")]
    DuplicatePath(String),
    #[error("[FRONTEND_NO_ENTRY] frontend entry '{0}' is not one of the listed files")]
    NoEntry(String),
    #[error(
        "[FRONTEND_UNSUPPORTED_TYPE] frontend file '{0}' has a file type the node does not serve"
    )]
    UnsupportedType(String),
    #[error(
        "[FRONTEND_TOO_LARGE] frontend files declare more than the {MAX_FRONTEND_BYTES} byte cap"
    )]
    TooLarge,
    #[error("[FRONTEND_SIZE_MISMATCH] frontend file '{path}' is {actual} bytes, the manifest says {declared}")]
    SizeMismatch {
        path: String,
        declared: u64,
        actual: u64,
    },
    #[error("[FRONTEND_HASH_MISMATCH] frontend file '{0}' does not match its manifest hash")]
    HashMismatch(String),
    #[error("[FRONTEND_REMOTE_RESOURCE] frontend file '{path}' loads '{url}' from another origin; the served page only allows its own files, ship it in the bundle")]
    RemoteResource { path: String, url: String },
    #[error("[FRONTEND_INVALID_HTML] frontend file '{0}' is not UTF-8")]
    InvalidHtml(String),
    #[error("[FRONTEND_ESCAPING_URL] frontend file '{0}' has a <base>; <base> changes how every relative URL resolves; remove it")]
    Base(String),
    #[error("[FRONTEND_ESCAPING_URL] frontend file '{path}' loads '{url}', outside the bundle's own folder; build with a relative base (Vite `base: './'`)")]
    EscapingUrl { path: String, url: String },
    #[error("[FRONTEND_INLINE_SCRIPT] frontend file '{path}' runs inline code ({what}); the served page forbids it, move it to a script file")]
    InlineScript { path: String, what: String },
    #[error("[FRONTEND_ENTRY_NOT_HTML] frontend entry '{0}' is not an HTML file")]
    EntryNotHtml(String),
    #[error("[FRONTEND_MISSING_FILE] frontend file '{0}' is listed in the manifest but is not in the archive or build output")]
    MissingFile(String),
}

impl FrontendError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsafePath(_) => "FRONTEND_UNSAFE_PATH",
            Self::DuplicatePath(_) => "FRONTEND_DUPLICATE_PATH",
            Self::NoEntry(_) => "FRONTEND_NO_ENTRY",
            Self::UnsupportedType(_) => "FRONTEND_UNSUPPORTED_TYPE",
            Self::TooLarge => "FRONTEND_TOO_LARGE",
            Self::SizeMismatch { .. } => "FRONTEND_SIZE_MISMATCH",
            Self::HashMismatch(_) => "FRONTEND_HASH_MISMATCH",
            Self::RemoteResource { .. } => "FRONTEND_REMOTE_RESOURCE",
            Self::InvalidHtml(_) => "FRONTEND_INVALID_HTML",
            Self::EscapingUrl { .. } | Self::Base(_) => "FRONTEND_ESCAPING_URL",
            Self::InlineScript { .. } => "FRONTEND_INLINE_SCRIPT",
            Self::EntryNotHtml(_) => "FRONTEND_ENTRY_NOT_HTML",
            Self::MissingFile(_) => "FRONTEND_MISSING_FILE",
        }
    }
}

impl BundleFrontend {
    /// The manifest-level rules, checked before any file bytes are read.
    pub fn validate(&self) -> Result<(), FrontendError> {
        let mut seen = BTreeSet::new();
        let mut total: u64 = 0;
        for file in &self.files {
            if !is_canonical_frontend_path(&file.path) {
                return Err(FrontendError::UnsafePath(file.path.clone()));
            }
            if !seen.insert(file.path.to_ascii_lowercase()) {
                return Err(FrontendError::DuplicatePath(file.path.clone()));
            }
            if mime_type(&file.path).is_none() {
                return Err(FrontendError::UnsupportedType(file.path.clone()));
            }
            total = total.saturating_add(file.size);
        }
        if total > MAX_FRONTEND_BYTES {
            return Err(FrontendError::TooLarge);
        }
        if !self.files.iter().any(|f| f.path == self.entry) {
            return Err(FrontendError::NoEntry(self.entry.clone()));
        }
        if mime_type(&self.entry) != Some(HTML_MIME) {
            return Err(FrontendError::EntryNotHtml(self.entry.clone()));
        }
        Ok(())
    }
}

/// Check one file's bytes against its signed manifest entry, and that an HTML
/// file loads no code from outside the bundle. Size first: the cap in
/// [`BundleFrontend::validate`] trusts the declared sizes.
pub fn verify_frontend_file(file: &BundleArtifact, bytes: &[u8]) -> Result<(), FrontendError> {
    let actual = bytes.len() as u64;
    if actual != file.size {
        return Err(FrontendError::SizeMismatch {
            path: file.path.clone(),
            declared: file.size,
            actual,
        });
    }
    if !hex::encode(Sha256::digest(bytes)).eq_ignore_ascii_case(&file.hash) {
        return Err(FrontendError::HashMismatch(file.path.clone()));
    }
    if mime_type(&file.path) == Some(HTML_MIME) {
        let html = std::str::from_utf8(bytes)
            .map_err(|_| FrontendError::InvalidHtml(file.path.clone()))?;
        check_html(&file.path, html)?;
    }
    Ok(())
}

/// Refuses what the served page's CSP or its `/apps/{package}/{version}/` path
/// would break. Only the tokenizer runs (linear time); a tree build is quadratic
/// on deep nesting. It decodes attributes and reads raw-text elements as a browser does.
fn check_html(path: &str, html: &str) -> Result<(), FrontendError> {
    let depth = path
        .strip_prefix(FRONTEND_ROOT)
        .unwrap_or(path)
        .matches('/')
        .count();
    let input = BufferQueue::default();
    input.push_back(StrTendril::from(html));
    let tokenizer = Tokenizer::new(HtmlCheck::new(path, depth), TokenizerOpts::default());
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    tokenizer.sink.error.take().map_or(Ok(()), Err)
}

struct HtmlCheck<'a> {
    path: &'a str,
    depth: usize,
    markup_only: Cell<bool>,
    open_scripts: RefCell<Vec<bool>>, // per open `<script>`: does its text run as code
    running: Cell<usize>,             // how many of them do
    error: RefCell<Option<FrontendError>>,
}

impl<'a> HtmlCheck<'a> {
    fn new(path: &'a str, depth: usize) -> Self {
        Self {
            path,
            depth,
            markup_only: Cell::new(false),
            open_scripts: RefCell::default(),
            running: Cell::new(0),
            error: RefCell::default(),
        }
    }

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
        if name == "script" && !(tag.self_closing && self.markup_only.get()) {
            let runs = attr(&attrs, "src").is_none() && is_executable(attr(&attrs, "type"));
            self.open_scripts.borrow_mut().push(runs);
            self.running.set(self.running.get() + usize::from(runs));
        }
        if MARKUP_ONLY_AFTER.contains(&name) {
            self.markup_only.set(true);
        }
        if self.markup_only.get() {
            return TokenSinkResult::Continue;
        }
        match RAW_TEXT.iter().find(|(raw, _)| *raw == name) {
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
            Token::TagToken(tag) if &*tag.name == "script" => {
                if self.open_scripts.borrow_mut().pop() == Some(true) {
                    self.running.set(self.running.get() - 1);
                }
            }
            Token::CharacterTokens(text) if self.running.get() > 0 && !text.trim().is_empty() => {
                self.inline_script("inline script");
            }
            Token::NullCharacterToken if self.running.get() > 0 => {
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

/// The `Content-Type` a frontend file is served with, `None` outside the allowlist.
pub fn mime_type(path: &str) -> Option<&'static str> {
    let (_, ext) = path
        .rsplit_once('/')
        .unwrap_or(("", path))
        .1
        .rsplit_once('.')?;
    MIME_TYPES
        .iter()
        .find(|(known, _)| ext.eq_ignore_ascii_case(known))
        .map(|(_, mime)| *mime)
}

/// One spelling per file, so the archive entry, the manifest and the served URL
/// can never name two different files.
fn is_canonical_frontend_path(path: &str) -> bool {
    path.strip_prefix(FRONTEND_ROOT).is_some_and(|rest| {
        rest.split('/').all(|part| {
            !matches!(part, "" | "." | "..")
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || PATH_PUNCTUATION.contains(c))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> BundleArtifact {
        BundleArtifact {
            path: path.to_owned(),
            hash: "00".to_owned(),
            size: 1,
        }
    }

    fn frontend(paths: &[&str]) -> BundleFrontend {
        BundleFrontend {
            entry: "frontend/index.html".to_owned(),
            files: std::iter::once("frontend/index.html")
                .chain(paths.iter().copied())
                .map(file)
                .collect(),
        }
    }

    #[test]
    fn refuses_a_path_outside_the_frontend_root_or_not_canonical() {
        for path in [
            "app.js",
            "frontend",
            "frontend/",
            "frontend/../manifest.json",
            "frontend/./app.js",
            "frontend//app.js",
            "frontend/a\\b.js",
            "/frontend/app.js",
            "./frontend/app.js",
            "frontend/a\nb.js",
            "frontend/a%2e%2e/x.js",
            "frontend/a.html#.js",
            "frontend/a?b.js",
            "frontend/a:b.js",
            "frontend/a b.js",
            "frontend/caf\u{e9}.js",
            "frontend/cafe\u{301}.js",
        ] {
            let err = frontend(&[path]).validate().unwrap_err();
            assert_eq!(err.code(), "FRONTEND_UNSAFE_PATH", "{path}");
        }
    }

    #[test]
    fn refuses_a_path_listed_twice_in_any_ascii_case() {
        for pair in [
            ["frontend/app.js", "frontend/app.js"],
            ["frontend/App.js", "frontend/app.js"],
        ] {
            let err = frontend(&pair).validate().unwrap_err();
            assert_eq!(err.code(), "FRONTEND_DUPLICATE_PATH", "{pair:?}");
        }
    }

    #[test]
    fn refuses_an_entry_that_is_not_html() {
        let mut fe = frontend(&["frontend/app.js"]);
        fe.entry = "frontend/app.js".to_owned();
        assert_eq!(fe.validate().unwrap_err().code(), "FRONTEND_ENTRY_NOT_HTML");
    }

    #[test]
    fn every_error_displays_its_own_code() {
        let s = || "x".to_owned();
        for err in [
            FrontendError::UnsafePath(s()),
            FrontendError::DuplicatePath(s()),
            FrontendError::NoEntry(s()),
            FrontendError::UnsupportedType(s()),
            FrontendError::TooLarge,
            FrontendError::SizeMismatch {
                path: s(),
                declared: 1,
                actual: 2,
            },
            FrontendError::HashMismatch(s()),
            FrontendError::RemoteResource {
                path: s(),
                url: s(),
            },
            FrontendError::Base(s()),
            FrontendError::InvalidHtml(s()),
            FrontendError::EscapingUrl {
                path: s(),
                url: s(),
            },
            FrontendError::InlineScript {
                path: s(),
                what: s(),
            },
            FrontendError::EntryNotHtml(s()),
            FrontendError::MissingFile(s()),
        ] {
            assert!(
                err.to_string().starts_with(&format!("[{}]", err.code())),
                "{err}"
            );
        }
    }

    #[test]
    fn refuses_an_entry_that_is_not_a_listed_file() {
        let mut fe = frontend(&[]);
        fe.entry = "frontend/missing.html".to_owned();
        assert_eq!(fe.validate().unwrap_err().code(), "FRONTEND_NO_ENTRY");
    }

    #[test]
    fn refuses_a_file_type_the_node_will_not_serve() {
        for path in ["frontend/app.js.map", "frontend/run.sh", "frontend/LICENSE"] {
            let err = frontend(&[path]).validate().unwrap_err();
            assert_eq!(err.code(), "FRONTEND_UNSUPPORTED_TYPE", "{path}");
        }
    }

    #[test]
    fn refuses_files_whose_declared_sizes_exceed_the_cap() {
        let mut fe = frontend(&["frontend/a.js", "frontend/b.js"]);
        fe.files[1].size = 16 * 1024 * 1024;
        fe.files[2].size = 16 * 1024 * 1024;
        assert_eq!(fe.validate().unwrap_err().code(), "FRONTEND_TOO_LARGE");

        fe.files[2].size = u64::MAX;
        assert_eq!(fe.validate().unwrap_err().code(), "FRONTEND_TOO_LARGE");
    }

    #[test]
    fn mime_types_are_looked_up_case_insensitively() {
        assert_eq!(
            mime_type("frontend/INDEX.HTML"),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(
            mime_type("frontend/a.mjs"),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(mime_type("frontend/a.map"), None);
    }

    fn declared(path: &str, bytes: &[u8]) -> BundleArtifact {
        BundleArtifact {
            path: path.to_owned(),
            hash: hex::encode(Sha256::digest(bytes)),
            size: bytes.len() as u64,
        }
    }

    #[test]
    fn verify_refuses_bytes_that_differ_from_the_manifest() {
        let mut art = declared("frontend/app.js", b"console.log(1)");
        verify_frontend_file(&art, b"console.log(1)").unwrap();

        let err = verify_frontend_file(&art, b"console.log(2)").unwrap_err();
        assert_eq!(err.code(), "FRONTEND_HASH_MISMATCH");

        art.size = 3;
        let err = verify_frontend_file(&art, b"console.log(1)").unwrap_err();
        assert_eq!(err.code(), "FRONTEND_SIZE_MISMATCH");
    }

    fn verify_html(html: &str) -> Result<(), FrontendError> {
        verify_frontend_file(
            &declared("frontend/index.html", html.as_bytes()),
            html.as_bytes(),
        )
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
            let err = verify_html(&format!("<!doctype html><head>{tag}</head>")).unwrap_err();
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
            let err = verify_html(&tag).unwrap_err();
            assert_eq!(err.code(), "FRONTEND_ESCAPING_URL", "{url}");
        }
        for tag in [
            r#"<link rel="modulepreload" href="/c.js">"#,
            r#"<link rel="icon" href="/vite.svg">"#,
            r#"<img src="/a.png">"#,
            r#"<img srcset="a.png 1x, /b.png 2x">"#,
            r#"<video poster="../../p.png"></video>"#,
        ] {
            let err = verify_html(tag).unwrap_err();
            assert_eq!(err.code(), "FRONTEND_ESCAPING_URL", "{tag}");
        }
    }

    #[test]
    fn refuses_any_base_element() {
        for tag in [
            r#"<base href="./">"#,
            r#"<base href="https://cdn.example/">"#,
        ] {
            let err = verify_html(tag).unwrap_err();
            assert_eq!(err.code(), "FRONTEND_ESCAPING_URL", "{tag}");
            assert!(err.to_string().contains("<base>"), "{err}");
        }
    }

    #[test]
    fn refuses_data_urls_on_code_loaders_only() {
        let err =
            verify_html(r#"<link rel="modulepreload" href="data:text/javascript,1">"#).unwrap_err();
        assert_eq!(err.code(), "FRONTEND_REMOTE_RESOURCE");
    }

    #[test]
    fn accepts_deeply_nested_html_quickly() {
        let start = std::time::Instant::now();
        verify_html(&"<span>".repeat(100_000)).unwrap();
        assert!(start.elapsed().as_secs() < 5, "{:?}", start.elapsed());
    }

    #[test]
    fn checks_deeply_nested_divs_in_linear_time() {
        let start = std::time::Instant::now();
        verify_html(&"<div>".repeat(200_000)).unwrap();
        assert!(start.elapsed().as_secs() < 2, "{:?}", start.elapsed());
    }

    #[test]
    fn raw_text_elements_hide_their_content_from_the_scan() {
        verify_html(
            r#"<title><img src="https://x/a.png"></title>
               <style><img src="https://x/a.png"></style>
               <textarea><img src="https://x/a.png"></textarea>
               <script type="text/template"><script src="https://x/a.js"></script>"#,
        )
        .unwrap();
    }

    #[test]
    fn foreign_content_and_select_are_scanned_as_markup() {
        for html in [
            r#"<svg><script href="a.js"><img src="https://x/a.png"></script></svg>"#,
            r#"<select><xmp></select><img src="https://x/a.png"></xmp>"#,
        ] {
            assert_eq!(
                verify_html(html).unwrap_err().code(),
                "FRONTEND_REMOTE_RESOURCE"
            );
        }
        let err = verify_html("<svg><script><b></b>x()</script></svg>").unwrap_err();
        assert_eq!(err.code(), "FRONTEND_INLINE_SCRIPT");
        verify_html("<svg><script href=a.js /></svg><p>text</p>").unwrap();
    }

    #[test]
    fn an_unclosed_script_still_counts_as_inline_code() {
        let err = verify_html("<script>alert(1)").unwrap_err();
        assert_eq!(err.code(), "FRONTEND_INLINE_SCRIPT");
    }

    #[test]
    fn climbing_is_measured_from_the_html_files_own_folder() {
        let html = r#"<script src="../app.js"></script>"#;
        let art = declared("frontend/sub/index.html", html.as_bytes());
        verify_frontend_file(&art, html.as_bytes()).unwrap();
        let html = r#"<script src="../../app.js"></script>"#;
        let art = declared("frontend/sub/index.html", html.as_bytes());
        let err = verify_frontend_file(&art, html.as_bytes()).unwrap_err();
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
        ] {
            let err = verify_html(&format!("<!doctype html>{html}")).unwrap_err();
            assert_eq!(err.code(), "FRONTEND_INLINE_SCRIPT", "{html}");
        }
    }

    #[test]
    fn allows_data_blocks_and_empty_script_elements() {
        verify_html(
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
        let err = verify_frontend_file(&declared("frontend/index.html", bytes), bytes).unwrap_err();
        assert_eq!(err.code(), "FRONTEND_INVALID_HTML");
    }

    #[test]
    fn accepts_html_that_loads_code_from_the_bundle() {
        verify_html(
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

    #[test]
    fn accepts_a_well_formed_frontend() {
        frontend(&["frontend/assets/app.js", "frontend/style.css"])
            .validate()
            .unwrap();
    }
}
