use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::events;

pub mod jsonrpc;

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
