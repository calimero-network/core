use serde::{Deserialize, Serialize};

/// Client ID is a locally unique identifier of a WebSocket client connection.
pub type ConnectionId = u64;
/// Request Id is a locally unique identifier of a WebSocket request.
pub type RequestId = u64;

// **************************** request *******************************
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Request<P> {
    pub id: Option<RequestId>,
    #[serde(flatten)]
    pub payload: P,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RequestPayload {
    Subscribe(SubscribeRequest),
    Unsubscribe(UnsubscribeRequest),
}
// *************************************************************************

// **************************** response *******************************
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Response {
    pub id: Option<RequestId>,
    #[serde(flatten)]
    pub body: ResponseBody,
}

impl Response {
    #[must_use]
    pub const fn new(id: Option<RequestId>, body: ResponseBody) -> Self {
        Self { id, body }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::exhaustive_enums)]
pub enum ResponseBody {
    Result(serde_json::Value),
    Error(ResponseBodyError),
}

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

// **************************** subscribe method *******************************
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SubscribeRequest {
    pub context_ids: Vec<calimero_primitives::context::ContextId>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SubscribeResponse {
    pub context_ids: Vec<calimero_primitives::context::ContextId>,
}

impl SubscribeResponse {
    #[must_use]
    pub fn new(context_ids: Vec<calimero_primitives::context::ContextId>) -> Self {
        Self { context_ids }
    }
}
// *************************************************************************

// **************************** unsubscribe method *******************************
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct UnsubscribeRequest {
    pub context_ids: Vec<calimero_primitives::context::ContextId>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct UnsubscribeResponse {
    pub context_ids: Vec<calimero_primitives::context::ContextId>,
}

impl UnsubscribeResponse {
    #[must_use]
    pub fn new(context_ids: Vec<calimero_primitives::context::ContextId>) -> Self {
        Self { context_ids }
    }
}
// *************************************************************************

#[derive(Debug)]
#[non_exhaustive]
pub enum Command {
    Close(u16, String),
    Send(Response),
}
