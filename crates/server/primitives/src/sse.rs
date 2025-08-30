use calimero_primitives::context::ContextId;
use eyre::Error as EyreError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Client ID is a locally unique identifier of a Sse client connection.
pub type ConnectionId = u64;

#[derive(Debug)]
pub enum Command {
    Close(String),
    Send(Response),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeRequest {
    pub context_id: Vec<ContextId>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    #[serde(flatten)]
    pub body: ResponseBody,
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
pub enum ResponseBodyError {
    ServerError(ServerResponseError),
    HandlerError(Value),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum ServerResponseError {
    ParseError(String),
    InternalError {
        #[serde(skip)]
        err: Option<EyreError>,
    },
}

#[derive(Debug)]
pub enum SseEvent {
    Message,
    Close,
    Error,
}

impl SseEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            SseEvent::Message => "message",
            SseEvent::Close => "close",
            SseEvent::Error => "error",
        }
    }
}
