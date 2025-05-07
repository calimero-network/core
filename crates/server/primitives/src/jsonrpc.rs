// use calimero_context_primitives::messages::execute::ExecuteError;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use eyre::Error as EyreError;
use serde::de::Error as SerdeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use thiserror::Error as ThisError;

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum RequestId {
    String(String),
    Number(u64),
    Null,
}

#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub enum Version {
    #[default]
    TwoPointZero,
}

impl Serialize for Version {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            Self::TwoPointZero => serializer.serialize_str("2.0"),
        }
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let version_str = String::deserialize(deserializer)?;
        match version_str.as_str() {
            "2.0" => Ok(Self::TwoPointZero),
            _ => Err(SerdeError::custom("Invalid JSON-RPC version")),
        }
    }
}

// **************************** request *******************************
#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Request<P> {
    pub jsonrpc: Version,
    pub id: Option<RequestId>,
    #[serde(flatten)]
    pub payload: P,
}

impl Request<RequestPayload> {
    #[must_use]
    pub const fn new(jsonrpc: Version, id: Option<RequestId>, payload: RequestPayload) -> Self {
        Self {
            jsonrpc,
            id,
            payload,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum RequestPayload {
    Execute(ExecuteRequest),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Response {
    pub jsonrpc: Version,
    pub id: Option<RequestId>,
    #[serde(flatten)]
    pub body: ResponseBody,
}

impl Response {
    #[must_use]
    pub const fn new(jsonrpc: Version, id: Option<RequestId>, body: ResponseBody) -> Self {
        Self { jsonrpc, id, body }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::exhaustive_enums,
    reason = "This will never have any other variants"
)]
pub enum ResponseBody {
    Result(ResponseBodyResult),
    Error(ResponseBodyError),
}

#[derive(Debug, Deserialize, Serialize)]
#[expect(
    clippy::exhaustive_structs,
    reason = "This will never have any other fields"
)]
pub struct ResponseBodyResult(pub Value);

#[derive(Debug, Deserialize, Serialize, ThisError)]
#[serde(untagged)]
#[non_exhaustive]
pub enum ResponseBodyError {
    #[error(transparent)]
    ServerError(ServerResponseError),
    #[error("handler error: {0}")]
    HandlerError(Value),
}

#[derive(Debug, Deserialize, Serialize, ThisError)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum ServerResponseError {
    #[error("parse error: {0}")]
    ParseError(String),
    #[error(
        "internal error: {}",
        err.as_ref().map_or_else(|| "<opaque>".to_owned(), ToString::to_string)
    )]
    InternalError {
        #[serde(skip)]
        err: Option<EyreError>,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ExecuteRequest {
    pub context_id: ContextId,
    pub method: String,
    pub args_json: Value,
    pub executor_public_key: PublicKey,
}

impl ExecuteRequest {
    #[must_use]
    pub const fn new(
        context_id: ContextId,
        method: String,
        args_json: Value,
        executor_public_key: PublicKey,
    ) -> Self {
        Self {
            context_id,
            method,
            args_json,
            executor_public_key,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ExecuteResponse {
    pub output: Option<Value>,
}

impl ExecuteResponse {
    #[must_use]
    pub const fn new(output: Option<Value>) -> Self {
        Self { output }
    }
}

#[derive(Debug, Deserialize, Serialize, ThisError)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum ExecuteError {
    #[error("codec error: {message}")]
    SerdeError { message: String },
    #[error("error occurred while handling request: {0}")]
    CallError(CallError),
    #[error("function call error: {0}")]
    FunctionCallError(String),
}
