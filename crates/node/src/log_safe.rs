//! What an always-on log line may say about an error whose words are not the node's.
//!
//! `merod run` logs at INFO, and on a fleet node every line ships to the
//! operator's log store (review finding F6). An error's words can quote what it
//! failed on: serde_json's `invalid type: string "alice@example.com", expected
//! u64` quotes the input, a peer's error quotes its answer, an upstream's quotes
//! its body. A log line names such an error by what the node knows of it (a
//! category, a position, a code, an I/O kind); the words go to the caller who
//! caused it, or to `debug!`. `scripts/check-no-user-data-at-info.py` refuses
//! the words at `info!`, `warn!` and `error!`;
//! `scripts/check-no-user-data-at-info.allow` lists the lines whose text is the
//! node's own.
//!
//! An `ApiError` is logged as its status and code, never its message. A
//! `StateTx::fail` reason ends up in the group's stop reason and its log lines,
//! so a decode failure passes [`serde()`], never the error itself.

use std::error::Error;

use crate::error::ApiError;

/// A serde_json error by its category and position, never its words.
#[must_use]
pub fn serde(e: &serde_json::Error) -> String {
    let category = match e.classify() {
        serde_json::error::Category::Io => "io",
        serde_json::error::Category::Syntax => "syntax",
        serde_json::error::Category::Data => "data",
        serde_json::error::Category::Eof => "eof",
    };
    // A value decoded from a `Value`, not from bytes, has no position (line 0).
    if e.line() == 0 {
        return format!("a JSON {category} error");
    }
    format!(
        "a JSON {category} error at line {} column {}",
        e.line(),
        e.column()
    )
}

/// An error by the first of its causes the node can name: an API error's code,
/// a JSON error's category, an I/O error's kind, an HTTP client error's kind;
/// `other` for any other.
#[must_use]
pub fn kind(e: &(dyn Error + 'static)) -> String {
    let mut cause = Some(e);
    while let Some(c) = cause {
        if let Some(api) = c.downcast_ref::<ApiError>() {
            return format!("{} {}", api.status, api.code);
        }
        if let Some(json) = c.downcast_ref::<serde_json::Error>() {
            return serde(json);
        }
        if let Some(io) = c.downcast_ref::<std::io::Error>() {
            return format!("io {:?}", io.kind());
        }
        if let Some(http) = c.downcast_ref::<reqwest::Error>() {
            return http_kind(http);
        }
        cause = c.source();
    }
    "other".to_owned()
}

fn http_kind(e: &reqwest::Error) -> String {
    if let Some(status) = e.status() {
        return format!("http status {}", status.as_u16());
    }
    let kind = if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "connect"
    } else if e.is_redirect() {
        "redirect"
    } else if e.is_body() || e.is_decode() {
        "body"
    } else if e.is_request() {
        "request"
    } else {
        "other"
    };
    format!("http {kind}")
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io;
    use std::sync::{Arc, Mutex, PoisonError};

    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl MakeWriter<'_> for Captured {
        type Writer = Self;

        fn make_writer(&self) -> Self {
            self.clone()
        }
    }

    /// What `f` logs at INFO and above (merod's production level), and its result.
    pub(crate) fn logged<R>(f: impl FnOnce() -> R) -> (R, String) {
        let out = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(out.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();
        let r = tracing::subscriber::with_default(subscriber, f);
        let bytes = out.0.lock().unwrap().clone();
        (r, String::from_utf8(bytes).unwrap())
    }

    const INPUT: &str = r#"{"index":"alice@example.com"}"#;

    #[derive(Debug, serde::Deserialize)]
    #[allow(dead_code, reason = "decoded only to fail")]
    struct Row {
        index: u64,
    }

    #[test]
    fn a_json_error_is_named_without_its_input() {
        let e = serde_json::from_str::<Row>(INPUT).unwrap_err();
        assert!(
            e.to_string().contains("alice"),
            "serde_json quotes the input: {e}"
        );
        let named = serde(&e);
        assert!(!named.contains("alice"), "{named}");
        assert!(named.starts_with("a JSON data error at line 1"), "{named}");

        let wrapped = eyre::Report::new(e).wrap_err("reading a row");
        let named = kind(wrapped.as_ref());
        assert!(!named.contains("alice"), "{named}");
        assert!(named.starts_with("a JSON data error"), "{named}");
    }

    #[test]
    fn an_api_error_is_named_by_its_code() {
        let e = ApiError::conflict("the name alice@example.com is taken");
        let named = kind(&e);
        assert_eq!(named, "409 CONFLICT");
        assert!(!named.contains("alice"));
        let other = io::Error::other("alice@example.com");
        assert_eq!(kind(&other), "io Other");
    }
}
