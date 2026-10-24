use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::BundleArtifact;

pub const FRONTEND_ROOT: &str = "frontend/"; // every frontend file's archive path starts here
pub const MAX_FRONTEND_BYTES: u64 = 32 * 1024 * 1024; // sum of every frontend file's declared size
const HTML_MIME: &str = "text/html; charset=utf-8"; // the one type an entry may have
const PATH_PUNCTUATION: &str = "._~@+-"; // the only non-alphanumerics a path component may hold

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
    ("webm", "video/webm"),
    ("xml", "application/xml"),
];

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

/// Check one file's bytes against its signed manifest entry, size first; the node calls this at install.
/// The caller must bound its read to `size + 1` bytes, as `bytes` is hashed whole.
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
    Ok(())
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
        assert_eq!(mime_type("frontend/a.webm"), Some("video/webm"));
        assert_eq!(mime_type("frontend/a.XML"), Some("application/xml"));
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

    #[test]
    fn accepts_a_well_formed_frontend() {
        frontend(&[
            "frontend/assets/app.js",
            "frontend/style.css",
            "frontend/clip.webm",
            "frontend/feed.xml",
        ])
        .validate()
        .unwrap();
    }

    #[cfg(feature = "html")]
    #[test]
    fn install_verification_does_not_read_the_html() {
        let html = br#"<script src="https://cdn.example/x.js"></script><body onload="x()">"#;
        verify_frontend_file(&declared("frontend/index.html", html), html).unwrap();
        let err = crate::check_frontend_html("frontend/index.html", html).unwrap_err();
        assert_eq!(err.code(), "FRONTEND_REMOTE_RESOURCE");
    }
}
