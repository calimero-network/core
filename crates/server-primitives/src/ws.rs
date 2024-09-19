use calimero_primitives::context::ContextId;
use eyre::Error as EyreError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
#[expect(
    clippy::exhaustive_enums,
    reason = "This will only ever have these variants"
)]
pub enum ResponseBody {
    Result(Value),
    Error(ResponseBodyError),
}

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

// **************************** subscribe method *******************************
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SubscribeRequest {
    pub context_ids: Vec<ContextId>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SubscribeResponse {
    pub context_ids: Vec<ContextId>,
}

impl SubscribeResponse {
    #[must_use]
    pub const fn new(context_ids: Vec<ContextId>) -> Self {
        Self { context_ids }
    }
}
// *************************************************************************

// **************************** unsubscribe method *******************************
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct UnsubscribeRequest {
    pub context_ids: Vec<ContextId>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct UnsubscribeResponse {
    pub context_ids: Vec<ContextId>,
}

impl UnsubscribeResponse {
    #[must_use]
    pub const fn new(context_ids: Vec<ContextId>) -> Self {
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
