//! Error types: the app-facing [`AppError`] and the SDK's own [`StorageError`] and [`TableError`].

use serde_json::json;
use serde_json::Value;
use thiserror::Error;

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
    /// A table has no index, unique constraint or count of this name.
    #[error("table has no `{0}`")]
    UnknownName(String),
    /// An update changed the record's key, which names the record.
    #[error("an update must not change the record key")]
    KeyChanged,
    /// An index entry points at a record that is gone: the index has drifted.
    #[error("index entry without a record")]
    Dangling,
    /// A counter's stored value is negative or not an integer.
    #[error("counter `{0}` does not hold a non-negative integer")]
    BadCounter(String),
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
    fn from(e: StorageError) -> Self {
        AppError::with_fields("Storage", json!({ "message": e.to_string() }))
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
