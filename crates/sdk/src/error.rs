//! Error types: the app-facing [`AppError`] and the SDK's own [`StorageError`] and [`TableError`].
//!
//! Ids in these comments cite `docs/specs/app-runtime-v0.14.md` (D decisions in section 2): D20
//! (64-bit integers travel as decimal strings in JSON), D50 (per-run limits on events, keys,
//! cursors and logs).

use std::io;

use serde_json::json;
use serde_json::Value;
use thiserror::Error;

use crate::files::FileIoError;

/// A typed app error: a code plus fields, as the ABI declares them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppError {
    /// The error code (the variant name of the app's error enum).
    pub code: String,
    /// The typed fields of the error.
    pub fields: Value,
}

impl AppError {
    /// An error with no fields.
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            fields: Value::Null,
        }
    }

    /// An error with fields.
    pub fn with_fields(code: impl Into<String>, fields: Value) -> Self {
        Self {
            code: code.into(),
            fields,
        }
    }

    /// The caller is not an account (`guards::require_member`).
    pub fn login_required() -> Self {
        Self::new("LoginRequired")
    }

    /// The caller's role is below the one the method needs.
    pub fn forbidden() -> Self {
        Self::new("Forbidden")
    }
}

impl core::fmt::Display for AppError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.fields.is_null() {
            write!(f, "{}", self.code)
        } else {
            write!(f, "{} {}", self.code, self.fields)
        }
    }
}

impl core::error::Error for AppError {}

/// The result type of app methods.
pub type Result<T> = core::result::Result<T, AppError>;

/// A failure of the storage layer: encoding, decoding, limits.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum StorageError {
    /// A value could not be encoded as deterministic CBOR.
    #[error("cannot encode value: {0}")]
    Encode(String),
    /// A stored value could not be decoded.
    #[error("cannot decode value: {0}")]
    Decode(String),
    /// NaN has no deterministic form and is refused.
    #[error("NaN is not allowed in stored values")]
    Nan,
    /// A key is longer than the host accepts (D50).
    #[error("key is {len} bytes, over the {max} byte limit")]
    KeyTooLong {
        /// The offending length.
        len: usize,
        /// The host's limit.
        max: usize,
    },
    /// A find or count prefix names more parts than the index or count has fields.
    #[error("prefix has {given} parts, the index has {max} fields")]
    PrefixTooLong {
        /// Parts given.
        given: usize,
        /// Fields declared.
        max: usize,
    },
    /// A page cursor is not one this SDK produced.
    #[error("malformed cursor")]
    BadCursor,
    /// The host refused a search: no search fields on the table, or a bad query.
    #[error("search: {0}")]
    Search(String),
    /// A table has no index, unique constraint or count of this name.
    #[error("table has no `{0}`")]
    UnknownName(String),
    /// An update changed the record's key, which names the record.
    #[error("an update must not change the record key")]
    KeyChanged,
    /// An index entry points at a record that is gone: the index has drifted.
    #[error("index entry without a record")]
    Dangling,
    /// A list has no item with this id.
    #[error("list has no item `{0}`")]
    UnknownItem(String),
    /// A counter's stored value is negative or not an integer.
    #[error("counter `{0}` does not hold a non-negative integer")]
    BadCounter(String),
    /// A position past the end of a vector or text.
    #[error("index {index} is out of bounds for length {len}")]
    OutOfBounds {
        /// The position asked for.
        index: u64,
        /// The length at the time.
        len: u64,
    },
    /// A text has no character with this id.
    #[error("no character with id {0}")]
    UnknownId(u64),
    /// The host refused a write past one of the run's limits (WIT `put-checked`, 0.5.7):
    /// nothing was written. Every node answers the same for the same run. An app that answers
    /// `Ok` after it keeps the run's earlier writes, so pass it on (`?`) unless the method
    /// means to commit them; split the work over several calls.
    #[error("the run passed its {limit} limit: {attempted}, over {max}")]
    RunLimit {
        /// Which limit.
        limit: RunLimit,
        /// The limit: 10,000 writes or 1 MiB.
        max: u64,
        /// What the run would have reached: the write's number, or its bytes with it.
        attempted: u64,
    },
    /// One edit would pass a run's write limits (`MAX_RUN_WRITES`, `MAX_RUN_BYTES`), so it
    /// was refused before any write; split it over several calls.
    #[error(
        "the edit needs {writes} writes of {bytes} bytes, over a run's 10,000 writes or 1 MiB"
    )]
    EditTooLarge {
        /// The writes it would make.
        writes: usize,
        /// The bytes it would write, as the host counts them.
        bytes: usize,
    },
}

/// Which of a run's write limits a write passed ([`StorageError::RunLimit`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunLimit {
    /// 10,000 writes (`put`, `delete`, `add`) per run.
    Writes,
    /// 1 MiB written per run: keys, values, events, xcalls.
    Bytes,
}

impl RunLimit {
    /// The name in errors and in `AppError` `RunLimit`'s `limit` field.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Writes => "writes",
            Self::Bytes => "bytes",
        }
    }
}

impl core::fmt::Display for RunLimit {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A failure of a table operation.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TableError {
    /// A unique index, or the record key (`index` = `"key"`), is already taken.
    #[error("duplicate on index `{index}`")]
    Duplicate {
        /// The unique constraint that clashed.
        index: String,
    },
    /// Anything below the table: encoding, limits, cursors.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl From<StorageError> for AppError {
    /// `Storage` with the message, except a run limit, which is the typed `RunLimit` with
    /// `limit` (`"writes"` or `"bytes"`), `max` and `attempted` (decimal strings, D20).
    fn from(e: StorageError) -> Self {
        if let StorageError::RunLimit {
            limit,
            max,
            attempted,
        } = e
        {
            return AppError::with_fields(
                "RunLimit",
                json!({
                    "limit": limit.as_str(),
                    "max": max.to_string(),
                    "attempted": attempted.to_string(),
                }),
            );
        }
        AppError::with_fields("Storage", json!({ "message": e.to_string() }))
    }
}

impl AppError {
    /// A host's soft error as `code` with its message, except the run's bytes limit
    /// (`entry too large: the run would write <n> bytes, over <max>`, which `emit`, `xcall`
    /// and `start_action` answer past 1 MiB written), which is the typed `RunLimit`, as a
    /// refused storage write is.
    pub(crate) fn host_soft(code: &str, message: String) -> Self {
        let parsed = message
            .strip_prefix("entry too large: the run would write ")
            .and_then(|rest| rest.split_once(" bytes, over "))
            .and_then(|(n, max)| Some((n.parse().ok()?, max.parse().ok()?)));
        match parsed {
            Some((attempted, max)) => StorageError::RunLimit {
                limit: RunLimit::Bytes,
                max,
                attempted,
            }
            .into(),
            None => AppError::with_fields(code, json!({ "message": message })),
        }
    }
}

impl From<io::Error> for AppError {
    fn from(e: io::Error) -> Self {
        let code = if e.get_ref().is_some_and(|i| i.is::<FileIoError>()) {
            "Files"
        } else {
            "Io"
        };
        AppError::with_fields(code, json!({ "message": e.to_string() }))
    }
}

impl From<TableError> for AppError {
    fn from(e: TableError) -> Self {
        match e {
            TableError::Duplicate { index } => {
                AppError::with_fields("Duplicate", json!({ "index": index }))
            }
            TableError::Storage(s) => s.into(),
        }
    }
}
