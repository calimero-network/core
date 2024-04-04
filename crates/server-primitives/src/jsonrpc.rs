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
    Call(CallRequest),
    CallMut(CallMutRequest),
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
pub struct CallRequest {
    pub application_id: calimero_primitives::application::ApplicationId,
    pub method: String,
    pub args_json: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallResponse {
    pub output: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Error)]
#[error("CallError")]
pub enum CallError {
    SerdeError { message: String },
    ExecutionError { message: String },
}
// *************************************************************************

// **************************** call_mut method ****************************
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallMutRequest {
    pub application_id: calimero_primitives::application::ApplicationId,
    pub method: String,
    pub args_json: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallMutResponse {
    pub output: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Error)]
#[error("CallMut")]
pub enum CallMutError {
    SerdeError { message: String },
    ExecutionError { message: String },
}
// *************************************************************************
