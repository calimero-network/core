use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error<R> {
    #[error(transparent)]
    JsonError(#[from] serde_json::Error),

    #[error("Failed to fetch: {0}")]
    FetchError(String),

    #[error("Server error: {0}")]
    ServerError(RpcError<R>),
}

#[derive(Debug, serde::Deserialize, Clone, PartialEq)]
#[serde(tag = "name", content = "cause", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RpcErrorKind<R> {
    RequestValidationError(RpcRequestValidationErrorKind),
    HandlerError(R),
    InternalError(Value),
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone, PartialEq)]
#[serde(tag = "name", content = "info", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RpcRequestValidationErrorKind {
    MethodNotFound { method_name: String },
    ParseError { error_message: String },
}

#[derive(Debug, serde::Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RpcError<T> {
    #[serde(flatten)]
    pub error_struct: Option<RpcErrorKind<T>>,
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}
