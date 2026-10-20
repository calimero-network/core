use std::collections::BTreeSet;
use std::sync::LazyLock;

use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::BundleArtifact;

const FRONTEND_ROOT: &str = "frontend/"; // every frontend file's archive path starts here
pub const MAX_FRONTEND_BYTES: u64 = 32 * 1024 * 1024; // sum of every frontend file's declared size
const HTML_MIME: &str = "text/html; charset=utf-8";

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
            if !seen.insert(file.path.as_str()) {
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
        if !seen.contains(self.entry.as_str()) {
            return Err(FrontendError::NoEntry(self.entry.clone()));
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
        if let Some(url) = remote_script_url(html) {
            return Err(FrontendError::RemoteScript {
                path: file.path.clone(),
                url,
            });
        }
    }
    Ok(())
}

/// The first URL an HTML document would load code from outside its own origin.
/// A spec parser decodes attributes and nests elements as a browser does.
fn remote_script_url(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    for element in document.select(&CODE_LOADERS) {
        let is_base = element.value().name() == "base";
        for (name, value) in element.value().attrs() {
            let loads = matches!(name, "src" | "href");
            if loads && (is_base || is_remote_url(value)) {
                return Some(value.to_owned());
            }
        }
    }
    None
}

/// Whether a browser would resolve `url` against something other than the
/// document's own origin: any scheme, or a scheme-relative `//host`. Follows the
/// URL spec's preprocessing, which trims spaces and drops tabs and newlines.
fn is_remote_url(url: &str) -> bool {
    let url: String = url
        .trim_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let mut chars = url.chars();
    let first_two = (chars.next(), chars.next());
    if matches!(first_two, (Some('/' | '\\'), Some('/' | '\\'))) {
        return true;
    }
    let scheme_len = url
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
        .unwrap_or(url.len());
    scheme_len > 0
        && url.starts_with(|c: char| c.is_ascii_alphabetic())
        && url[scheme_len..].starts_with(':')
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
                && !part.contains(|c: char| c == '\\' || c.is_control())
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
        ] {
            let err = frontend(&[path]).validate().unwrap_err();
            assert_eq!(err.code(), "FRONTEND_UNSAFE_PATH", "{path}");
        }
    }

    #[test]
    fn refuses_a_path_listed_twice() {
        let err = frontend(&["frontend/app.js", "frontend/app.js"])
            .validate()
            .unwrap_err();
        assert_eq!(err.code(), "FRONTEND_DUPLICATE_PATH");
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
    fn refuses_html_that_is_not_utf8() {
        let bytes = b"<script src=\"/a.js\"></script>\xff";
        let err = verify_frontend_file(&declared("frontend/index.html", bytes), bytes).unwrap_err();
        assert_eq!(err.code(), "FRONTEND_INVALID_HTML");
    }

    #[test]
    fn accepts_html_that_loads_code_from_the_bundle() {
        verify_html(
            r#"<!doctype html><head>
                <script type="module" src="/assets/app.js"></script>
                <script src="./vendor.js"></script>
                <link rel="modulepreload" href="assets/chunk.js">
                <a href="https://docs.example/">docs</a>
                <script>console.log("inline")</script>
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
