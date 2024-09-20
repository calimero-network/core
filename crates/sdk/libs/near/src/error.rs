use serde::{Deserialize, Serialize};
use serde_json::{Error as JsonError, Value};
use thiserror::Error as ThisError;

#[derive(Debug, ThisError)]
pub enum Error<R> {
    #[error(transparent)]
    JsonError(#[from] JsonError),

    #[error("Failed to fetch: {0}")]
    FetchError(String),

    #[error("Server error: {0}")]
    ServerError(RpcError<R>),
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "name", content = "cause", rename_all = "SCREAMING_SNAKE_CASE")]
#[expect(clippy::enum_variant_names)]
pub enum RpcErrorKind<R> {
    RequestValidationError(RpcRequestValidationErrorKind),
    HandlerError(R),
    InternalError(Value),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "name", content = "info", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RpcRequestValidationErrorKind {
    MethodNotFound { method_name: String },
    ParseError { error_message: String },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RpcError<T> {
    #[serde(flatten)]
    pub error_struct: Option<RpcErrorKind<T>>,
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}
