use std::collections::BTreeSet;
use std::sync::LazyLock;

use scraper::{ElementRef, Html, Selector};
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

/// Elements that load code, by every attribute a browser would load it from;
/// `base` re-points every relative URL in the document, so any value counts.
static CODE_LOADERS: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(r#"script, link[rel~="modulepreload"], link[rel~="preload"], base"#)
        // SAFETY: the selector is a constant, and a unit test parses it on every run
        .expect("static selector parses")
});

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
    #[error("[FRONTEND_REMOTE_SCRIPT] frontend file '{path}' loads code from '{url}'; ship it in the bundle")]
    RemoteScript { path: String, url: String },
    #[error("[FRONTEND_INVALID_HTML] frontend file '{0}' is not UTF-8")]
    InvalidHtml(String),
    #[error("[FRONTEND_ESCAPING_URL] frontend file '{path}' loads code from '{url}', outside the bundle's own folder; build with a relative base (Vite `base: './'`)")]
    EscapingUrl { path: String, url: String },
    #[error("[FRONTEND_INLINE_SCRIPT] frontend file '{path}' runs inline code ({what}); the served page forbids it, move it to a script file")]
    InlineScript { path: String, what: String },
    #[error("[FRONTEND_ENTRY_NOT_HTML] frontend entry '{0}' is not an HTML file")]
    EntryNotHtml(String),
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
            Self::RemoteScript { .. } => "FRONTEND_REMOTE_SCRIPT",
            Self::InvalidHtml(_) => "FRONTEND_INVALID_HTML",
            Self::EscapingUrl { .. } => "FRONTEND_ESCAPING_URL",
            Self::InlineScript { .. } => "FRONTEND_INLINE_SCRIPT",
            Self::EntryNotHtml(_) => "FRONTEND_ENTRY_NOT_HTML",
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
/// would break. A spec parser decodes attributes and nests elements as a browser does.
fn check_html(path: &str, html: &str) -> Result<(), FrontendError> {
    let document = Html::parse_document(html);
    let depth = path
        .strip_prefix(FRONTEND_ROOT)
        .unwrap_or(path)
        .matches('/')
        .count();
    for element in document.select(&CODE_LOADERS) {
        let is_base = element.value().name() == "base";
        for (name, value) in element.value().attrs() {
            if !matches!(name, "src" | "href") {
                continue;
            }
            let url = normalize_url(value);
            if is_base || is_remote_url(&url) {
                return Err(FrontendError::RemoteScript {
                    path: path.to_owned(),
                    url: value.to_owned(),
                });
            }
            if escapes_bundle(&url, depth) {
                return Err(FrontendError::EscapingUrl {
                    path: path.to_owned(),
                    url: value.to_owned(),
                });
            }
        }
    }
    for element in document
        .root_element()
        .descendants()
        .filter_map(ElementRef::wrap)
    {
        if let Some(what) = inline_code(element) {
            return Err(FrontendError::InlineScript {
                path: path.to_owned(),
                what,
            });
        }
    }
    Ok(())
}

/// What makes `element` run code the CSP forbids, if anything.
fn inline_code(element: ElementRef) -> Option<String> {
    let value = element.value();
    for (name, attr) in value.attrs() {
        let is_url = matches!(name, "href" | "src" | "action" | "formaction");
        if name.starts_with("on") {
            return Some(format!("event handler '{name}'"));
        }
        if is_url && has_scheme(&normalize_url(attr), "javascript") {
            return Some(format!("{name}=\"{attr}\""));
        }
    }
    let executable = value.attr("type").is_none_or(|t| {
        let essence = t
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        essence.is_empty() || SCRIPT_TYPES.contains(&essence.as_str())
    });
    let has_code = element.text().any(|t| !t.trim().is_empty());
    (value.name() == "script" && value.attr("src").is_none() && executable && has_code)
        .then(|| "inline script".to_owned())
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
            FrontendError::RemoteScript {
                path: s(),
                url: s(),
            },
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
            r#"<base href="https://cdn.example/">"#,
            r#"<svg><script href="https://cdn.example/x.js"></script></svg>"#,
            r#"<svg><script xlink:href="https://cdn.example/x.js"></script></svg>"#,
        ] {
            let err = verify_html(&format!("<!doctype html><head>{tag}</head>")).unwrap_err();
            assert_eq!(err.code(), "FRONTEND_REMOTE_SCRIPT", "{tag}");
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
        ] {
            let tag = format!(r#"<script src="{url}"></script>"#);
            let err = verify_html(&tag).unwrap_err();
            assert_eq!(err.code(), "FRONTEND_ESCAPING_URL", "{url}");
        }
        let tag = r#"<link rel="modulepreload" href="/c.js">"#;
        assert_eq!(
            verify_html(tag).unwrap_err().code(),
            "FRONTEND_ESCAPING_URL"
        );
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
               <script src="a.js">  </script>
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
