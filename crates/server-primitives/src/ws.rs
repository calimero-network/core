use serde::{Deserialize, Serialize};

/// Client ID is a locally unique identifier of a WebSocket client connection.
pub type ConnectionId = u64;
/// Request Id is a locally unique identifier of a WebSocket request.
pub type RequestId = u64;

// **************************** request *******************************
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub id: Option<RequestId>,
    #[serde(flatten)]
    pub payload: RequestPayload,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum RequestPayload {
    Subscribe(SubscribeRequest),
    Unsubscribe(UnsubscribeRequest),
}
// *************************************************************************

// **************************** response *******************************
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
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
#[serde(rename_all = "camelCase")]
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

// **************************** subscribe method *******************************
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeRequest {
    pub application_ids: Vec<calimero_primitives::application::ApplicationId>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeResponse {
    pub application_ids: Vec<calimero_primitives::application::ApplicationId>,
}
// *************************************************************************

// **************************** unsubscribe method *******************************
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeRequest {
    pub application_ids: Vec<calimero_primitives::application::ApplicationId>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeResponse {
    pub application_ids: Vec<calimero_primitives::application::ApplicationId>,
}
// *************************************************************************

pub enum Command {
    Close(u16, String),
    Send(Response),
}
