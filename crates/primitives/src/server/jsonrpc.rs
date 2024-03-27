use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// Request Id is a locally unique identifier of a WebSocket client connection.
pub type RequestId = u64;

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

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CallRequest {
    pub application_id: String,
    pub method: String,
    pub args_json: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CallResponse {
    pub output: Option<String>,
}

#[derive(Debug, Error, Serialize, Deserialize)]
#[error("CallError")]
pub enum CallError {
    SerdeError(String),
    ExecutionError(String),
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CallMutRequest {
    pub application_id: String,
    pub method: String,
    pub args_json: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CallMutResponse {
    pub output: Option<String>,
}

#[derive(Debug, Error, Serialize, Deserialize)]
#[error("CallMut")]
pub enum CallMutError {
    SerdeError(String),
    ExecutionError(String),
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "method", content = "params", rename_all = "camelCase")]
pub enum RequestPayload {
    Call(CallRequest),
    CallMut(CallMutRequest),
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub jsonrpc: Version,
    #[serde(flatten)]
    pub payload: RequestPayload,
    pub id: Option<RequestId>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum ResponseResult {
    Call(CallResponse),
    CallMut(CallMutResponse),
}

#[derive(Debug, Error, Serialize, Deserialize)]
#[error("ResponseError")]
pub enum ResponseError {
    Call(CallError),
    CallMut(CallMutError),
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub jsonrpc: Version,
    pub result: Option<ResponseResult>,
    pub error: Option<ResponseError>,
    pub id: Option<RequestId>,
}
