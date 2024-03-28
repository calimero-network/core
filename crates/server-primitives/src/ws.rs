use serde::{Deserialize, Serialize};

/// Client ID is a locally unique identifier of a WebSocket client connection.
pub type ClientId = u64;
/// Request Id is a locally unique identifier of a WebSocket request.
pub type RequestId = u64;

// **************************** request *******************************
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub id: Option<RequestId>,
    pub body: RequestBody,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RequestBody {
    Subscribe,
    Unsubscribe,
}
// *************************************************************************

// **************************** response *******************************
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub id: Option<RequestId>,
    pub body: ResonseBody,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResponseBodyResult {
    Subscribed,
    Unsubscribed,
    Event(calimero_primitives::events::NodeEvent),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResonseBody {
    Result(ResponseBodyResult),
    Error(ResponseBodyError),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResponseBodyError {
    SerdeError(String),
    ExecutionError(String),
}
// *************************************************************************

pub enum Command {
    Close(u16, String),
    Send(Response),
}
