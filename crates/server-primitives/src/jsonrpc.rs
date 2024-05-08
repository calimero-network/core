use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Number(u64),
    Null,
}

#[derive(Debug)]
pub enum Version {
    TwoPointZero,
}

impl Serialize for Version {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            Version::TwoPointZero => serializer.serialize_str("2.0"),
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
            "2.0" => Ok(Version::TwoPointZero),
            _ => Err(serde::de::Error::custom("Invalid JSON-RPC version")),
        }
    }
}

// **************************** request *******************************
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request<P> {
    pub jsonrpc: Version,
    pub id: Option<RequestId>,
    #[serde(flatten)]
    pub payload: P,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum RequestPayload {
    Query(QueryRequest),
    Mutate(MutateRequest),
}
// *************************************************************************

// **************************** response *******************************
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub jsonrpc: Version,
    pub id: Option<RequestId>,
    #[serde(flatten)]
    pub body: ResponseBody,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResponseBody {
    Result(ResponseBodyResult),
    Error(ResponseBodyError),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResponseBodyResult(pub serde_json::Value);

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseBodyError {
    ServerError(ServerResponseError),
    HandlerError(serde_json::Value),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ServerResponseError {
    ParseError(String),
    InternalError {
        #[serde(skip)]
        err: Option<eyre::Error>,
    },
}
// *************************************************************************

// **************************** call method *******************************
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryRequest {
    pub application_id: calimero_primitives::application::ApplicationId,
    pub method: String,
    pub args_json: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResponse {
    pub output: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Error)]
#[error("QueryError")]
#[serde(tag = "name", content = "cause")]
pub enum QueryError {
    SerdeError { message: String },
    CallError(calimero_node_primitives::CallError),
}
// *************************************************************************

// **************************** call_mut method ****************************
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutateRequest {
    pub application_id: calimero_primitives::application::ApplicationId,
    pub method: String,
    pub args_json: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutateResponse {
    pub output: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Error)]
#[error("MutateError")]
#[serde(tag = "name", content = "cause")]
pub enum MutateError {
    SerdeError { message: String },
    CallError(calimero_node_primitives::CallError),
}
// *************************************************************************
