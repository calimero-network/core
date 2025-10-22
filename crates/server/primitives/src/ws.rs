use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
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
pub struct Request<P> {
    pub id: Option<RequestId>,
    #[serde(flatten)]
    pub payload: P,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum RequestPayload {
    Subscribe(SubscribeRequest),
    Unsubscribe(UnsubscribeRequest),
    Execute(ExecuteRequest),
    Query(QueryRequest),
}
// *************************************************************************

// **************************** response *******************************
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub id: Option<RequestId>,
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
// *************************************************************************

// **************************** subscribe method *******************************
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeRequest {
    pub context_ids: Vec<ContextId>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeResponse {
    pub context_ids: Vec<ContextId>,
}
// *************************************************************************

// **************************** unsubscribe method *******************************
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeRequest {
    pub context_ids: Vec<ContextId>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeResponse {
    pub context_ids: Vec<ContextId>,
}
// *************************************************************************

// **************************** execute method *******************************
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteRequest {
    pub context_id: ContextId,
    pub method: String,
    pub args_json: Value,
    pub executor_public_key: PublicKey,
    #[serde(default)]
    pub substitute: Vec<Alias<PublicKey>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteResponse {
    pub output: Option<Value>,
}
// *************************************************************************

// **************************** query method *******************************
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryRequest {
    pub context_id: ContextId,
    pub method: String,
    pub args_json: Value,
    pub executor_public_key: PublicKey,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResponse {
    pub output: Option<Value>,
}
// *************************************************************************

#[derive(Debug)]
pub enum Command {
    Close(u16, String),
    Send(Response),
}
