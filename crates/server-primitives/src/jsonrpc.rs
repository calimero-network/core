use calimero_node_primitives::CallError;
use calimero_primitives::context::ContextId;
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
pub struct ResponseBodyResult(pub Value);

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum ResponseBodyError {
    ServerError(ServerResponseError),
    HandlerError(Value),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum ServerResponseError {
    ParseError(String),
    InternalError {
        #[serde(skip)]
        err: Option<EyreError>,
    },
}
// *************************************************************************

// **************************** call method *******************************
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct QueryRequest {
    pub context_id: ContextId,
    pub method: String,
    pub args_json: Value,
    pub executor_public_key: [u8; 32],
}

impl QueryRequest {
    #[must_use]
    pub fn new(
        context_id: ContextId,
        method: String,
        args_json: Value,
        executor_public_key: [u8; 32],
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
pub struct QueryResponse {
    pub output: Option<Value>,
}

impl QueryResponse {
    #[must_use]
    pub const fn new(output: Option<Value>) -> Self {
        Self { output }
    }
}

#[derive(Debug, Deserialize, Serialize, ThisError)]
#[error("QueryError")]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum QueryError {
    SerdeError { message: String },
    CallError(CallError),
    FunctionCallError(String),
}
// *************************************************************************

// **************************** call_mut method ****************************
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MutateRequest {
    pub context_id: ContextId,
    pub method: String,
    pub args_json: Value,
    pub executor_public_key: [u8; 32],
}

impl MutateRequest {
    #[must_use]
    pub fn new(
        context_id: ContextId,
        method: String,
        args_json: Value,
        executor_public_key: [u8; 32],
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
pub struct MutateResponse {
    pub output: Option<Value>,
}

impl MutateResponse {
    #[must_use]
    pub const fn new(output: Option<Value>) -> Self {
        Self { output }
    }
}

#[derive(Debug, Deserialize, Serialize, ThisError)]
#[error("MutateError")]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum MutateError {
    SerdeError { message: String },
    CallError(CallError),
    FunctionCallError(String),
}
// *************************************************************************
