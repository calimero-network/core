use calimero_context_primitives::messages::execute::ExecuteError;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum RequestId {
    String(String),
    Number(u64),
    #[default]
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
        match self {
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
            _ => Err(de::Error::custom("Invalid JSON-RPC version")),
        }
    }
}

// **************************** request *******************************
#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Request<P> {
    pub jsonrpc: Version,
    pub id: RequestId,
    #[serde(flatten)]
    pub payload: P,
}

impl Request<RequestPayload> {
    #[must_use]
    pub const fn new(jsonrpc: Version, id: RequestId, payload: RequestPayload) -> Self {
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
    Execute(ExecutionRequest),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Response {
    pub jsonrpc: Version,
    pub id: RequestId,
    #[serde(flatten)]
    pub body: ResponseBody,
}

impl Response {
    #[must_use]
    pub const fn new(jsonrpc: Version, id: RequestId, body: ResponseBody) -> Self {
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
pub struct ResponseBodyResult(pub serde_json::Value);

#[derive(Debug, Deserialize, Serialize, Error)]
#[serde(untagged)]
#[non_exhaustive]
pub enum ResponseBodyError {
    #[error(transparent)]
    ServerError(ServerResponseError),
    #[error("handler error: {0}")]
    HandlerError(serde_json::Value),
}

#[derive(Debug, Deserialize, Serialize, Error)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum ServerResponseError {
    #[error("parse error: {0}")]
    ParseError(String),
    #[error(
        "internal error: {}",
        err.as_ref().map_or_else(|| "<opaque>".to_owned(), |e| e.to_string())
    )]
    InternalError {
        #[serde(skip)]
        err: Option<eyre::Report>,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ExecutionRequest {
    pub context_id: ContextId,
    pub method: String,
    pub args_json: serde_json::Value,
    pub executor_public_key: PublicKey,
    #[serde(default)]
    pub substitute: Vec<Alias<PublicKey>>,
}

impl ExecutionRequest {
    #[must_use]
    pub const fn new(
        context_id: ContextId,
        method: String,
        args_json: serde_json::Value,
        executor_public_key: PublicKey,
        substitute: Vec<Alias<PublicKey>>,
    ) -> Self {
        Self {
            context_id,
            method,
            args_json,
            executor_public_key,
            substitute,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ExecutionResponse {
    pub output: Option<serde_json::Value>,
}

impl ExecutionResponse {
    #[must_use]
    pub const fn new(output: Option<serde_json::Value>) -> Self {
        Self { output }
    }
}

#[derive(Debug, Deserialize, Serialize, Error)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum ExecutionError {
    #[error("codec error: {message}")]
    SerdeError { message: String },
    #[error("function call error: {0}")]
    FunctionCallError(String),
    #[serde(untagged)]
    #[error(transparent)]
    ExecuteError(ExecuteError),
}
