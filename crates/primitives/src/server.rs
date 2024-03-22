use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::events;

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum WsRequestBody {
    Subscribe,
    Unsubscribe,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum WsResponseBodyResult {
    Subscribed,
    Unsubscribed,
    Event(events::NodeEvent),
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum WsResonseBody {
    Result(WsResponseBodyResult),
    Error(WsError),
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum WsError {
    SerdeError(String),
    ExecutionError(String),
}

// WebSocket API
/// Client ID is a locally unique identifier of a WebSocket client connection.
pub type WsClientId = u64;
/// Request Id is a locally unique identifier of a WebSocket client connection.
pub type WsRequestId = u64;

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WsRequest {
    pub id: Option<WsRequestId>,
    pub body: WsRequestBody,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WsResponse {
    pub id: Option<WsRequestId>,
    pub body: WsResonseBody,
}

pub enum WsCommand {
    Close(u16, String),
    Send(WsResponse),
}

#[derive(Debug)]
pub enum JsonRpcVersion {
    TwoPointZero,
}

impl Serialize for JsonRpcVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            JsonRpcVersion::TwoPointZero => serializer.serialize_str("2.0"),
        }
    }
}

impl<'de> Deserialize<'de> for JsonRpcVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let version_str = String::deserialize(deserializer)?;
        match version_str.as_str() {
            "2.0" => Ok(JsonRpcVersion::TwoPointZero),
            _ => Err(serde::de::Error::custom("Invalid JSON-RPC version")),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JsonRpcRequestCall {
    pub app_id: String,
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JsonRpcRequestParams {
    pub call: JsonRpcRequestCall,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum JsonRpcRequestParam2s {
    Read,
    Call,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JsonRpcRequest {
    pub jsonrpc: JsonRpcVersion,
    pub method: String,
    pub params: Option<JsonRpcRequestParams>,
    pub id: Option<WsRequestId>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JsonRpcResponseError {
    pub code: u64,
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JsonRpcResponse {
    pub jsonrpc: JsonRpcVersion,
    pub result: String,
    pub error: Option<JsonRpcResponseError>,
    pub id: Option<WsRequestId>,
}
