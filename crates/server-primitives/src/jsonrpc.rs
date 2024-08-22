use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum RequestId {
    String(String),
    Number(u64),
    Null,
}

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum Version {
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
            _ => Err(serde::de::Error::custom("Invalid JSON-RPC version")),
        }
    }
}

// **************************** request *******************************
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Request<P> {
    pub jsonrpc: Version,
    pub id: Option<RequestId>,
    #[serde(flatten)]
    pub payload: P,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RequestPayload {
    Query(QueryRequest),
    Mutate(MutateRequest),
}
// *************************************************************************

// **************************** response *******************************
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
#[allow(clippy::exhaustive_enums)]
pub enum ResponseBody {
    Result(ResponseBodyResult),
    Error(ResponseBodyError),
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(clippy::exhaustive_structs)]
pub struct ResponseBodyResult(pub serde_json::Value);

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum ResponseBodyError {
    ServerError(ServerResponseError),
    HandlerError(serde_json::Value),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum ServerResponseError {
    ParseError(String),
    InternalError {
        #[serde(skip)]
        err: Option<eyre::Error>,
    },
}
// *************************************************************************

// **************************** call method *******************************
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct QueryRequest {
    pub context_id: calimero_primitives::context::ContextId,
    pub method: String,
    pub args_json: serde_json::Value,
    pub executor_public_key: [u8; 32],
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct QueryResponse {
    pub output: Option<serde_json::Value>,
}

impl QueryResponse {
    #[must_use]
    pub const fn new(output: Option<serde_json::Value>) -> Self {
        Self { output }
    }
}

#[derive(Debug, Deserialize, Error, Serialize)]
#[error("QueryError")]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum QueryError {
    SerdeError { message: String },
    CallError(calimero_node_primitives::CallError),
    FunctionCallError(String),
}
// *************************************************************************

// **************************** call_mut method ****************************
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MutateRequest {
    pub context_id: calimero_primitives::context::ContextId,
    pub method: String,
    pub args_json: serde_json::Value,
    pub executor_public_key: [u8; 32],
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MutateResponse {
    pub output: Option<serde_json::Value>,
}

impl MutateResponse {
    #[must_use]
    pub const fn new(output: Option<serde_json::Value>) -> Self {
        Self { output }
    }
}

#[derive(Debug, Deserialize, Error, Serialize)]
#[error("MutateError")]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum MutateError {
    SerdeError { message: String },
    CallError(calimero_node_primitives::CallError),
    FunctionCallError(String),
}
// *************************************************************************
