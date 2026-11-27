//! One exchange with NEAR AI Cloud's chat completions (OpenAI-compatible) from an app's
//! [action](https://calimero-network.github.io/core/protocol/execution/#actions-component-apps):
//! [`chat`] posts a [`Prompt`] to [`ENDPOINT`] and parses the reply into an [`Answer`] or an
//! [`Error`].
//!
//! The request carries no key: the app's manifest binds `NEAR_AI_API_KEY` to this endpoint
//! and the node attaches it (README). What an app keeps, who may ask and how much stay the
//! app's.

use std::collections::BTreeMap;

use calimero_sdk::prelude::*;
use calimero_sdk::Request;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// NEAR AI Cloud's chat completions: the URL an app's manifest declares in `outbound`.
pub const ENDPOINT: &str = "https://cloud-api.near.ai/v1/chat/completions";

/// One chat message, as OpenAI-compatible APIs take it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, AbiType)]
#[serde(tag = "role", rename_all = "lowercase", deny_unknown_fields)]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: String,
    },
    /// `content` is `null` when the model only called tools.
    Assistant {
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<ToolCall>>,
    },
    /// A tool's result, answering the call `tool_call_id`.
    Tool {
        tool_call_id: String,
        content: String,
    },
}

/// A call the model made; `arguments` is the JSON text it wrote, kept as a string.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, AbiType)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: Function,
    pub function: FunctionCall,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, AbiType)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// The only kind of tool: `"function"`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, AbiType)]
#[serde(rename_all = "lowercase")]
pub enum Function {
    Function,
}

/// A tool the model may call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, AbiType)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    #[serde(rename = "type")]
    pub kind: Function,
    pub function: FunctionDef,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, AbiType)]
#[serde(deny_unknown_fields)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A JSON Schema object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<BTreeMap<String, Json>>,
}

/// `"auto"`, `"none"`, `"required"`, or `{ "type": "function", "function": { "name" } }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, AbiType)]
#[serde(untagged)]
pub enum ToolChoice {
    Mode(ToolMode),
    Named { r#type: Function, function: Named },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, AbiType)]
#[serde(rename_all = "lowercase")]
pub enum ToolMode {
    Auto,
    None,
    Required,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, AbiType)]
#[serde(deny_unknown_fields)]
pub struct Named {
    pub name: String,
}

/// Any JSON value, for a tool's schema: the ABI has no free-form type. Integers are tried
/// before floats so `80` goes out as `80`, not `80.0`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, AbiType)]
#[serde(untagged)]
#[abi(plain)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

/// What goes to NEAR AI: the request body, as is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, AbiType)]
pub struct Prompt {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    /// Sent only when set: some models refuse it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
}

/// The model's answer: its message, as the next request's messages take it back, why it
/// stopped, and what it cost.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, AbiType)]
pub struct Answer {
    /// Always `assistant`.
    pub message: Message,
    pub finish_reason: String,
    pub usage: Usage,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, AbiType)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Why [`chat`] has no answer. Into an [`AppError`] with code `NearAiNotSent`,
/// `NearAiStatus` or `NearAiUnusable` and these fields.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The request did not go out, or no response came back: the node's reason.
    #[error("not sent: {reason}")]
    NotSent { reason: String },
    /// NEAR AI answered an error status, with its error's message when it sent one.
    #[error("NEAR AI answered {status}{}", message.as_ref().map(|m| format!(": {m}")).unwrap_or_default())]
    Status {
        status: u16,
        message: Option<String>,
    },
    /// NEAR AI answered, but with nothing to use: no answer, or not a chat completion.
    #[error("{reason}")]
    Unusable { reason: String },
}

impl From<Error> for AppError {
    fn from(e: Error) -> Self {
        match e {
            Error::NotSent { reason } => {
                AppError::with_fields("NearAiNotSent", json!({ "reason": reason }))
            }
            Error::Status { status, message } => AppError::with_fields(
                "NearAiStatus",
                json!({ "status": status, "message": message }),
            ),
            Error::Unusable { reason } => {
                AppError::with_fields("NearAiUnusable", json!({ "reason": reason }))
            }
        }
    }
}

/// Sends `request` to NEAR AI and parses its reply.
///
/// # Errors
///
/// [`Error`]: not sent, an error status, or an answer with nothing to use.
pub fn chat(ctx: &mut ActionCtx, request: &Prompt) -> Result<Answer, Error> {
    let response = ctx
        .fetch(&http_request(request, &retry_scope(ctx)))
        .map_err(|e| Error::NotSent {
            reason: fetch_reason(e),
        })?;
    answer(response.status, &response.body)
}

/// The SDK's `Fetch` error carries its reason as `message`.
fn fetch_reason(e: AppError) -> String {
    match e.fields.get("message").and_then(Value::as_str) {
        Some(message) => message.to_owned(),
        None => e.to_string(),
    }
}

/// This app's action, as the node names it on every attempt.
fn retry_scope(ctx: &ActionCtx) -> String {
    format!("{}\0{}", env::app_id(), ctx.key())
}

/// The POST of `prompt`; no `authorization`: the node attaches the bound key.
/// The retry key hashes app, action and body: stable across retries, apart across apps and prompts.
fn http_request(prompt: &Prompt, scope: &str) -> Request {
    // SAFETY: every map key is a `String`, and serde_json writes a non-finite float as null.
    let body = serde_json::to_vec(prompt).expect("a prompt is JSON");
    let retry_key = hex::encode(
        Sha256::new()
            .chain_update(scope.as_bytes())
            .chain_update([0])
            .chain_update(&body)
            .finalize(),
    );
    Request::post(ENDPOINT, body)
        .header("content-type", "application/json")
        .header("idempotency-key", &retry_key)
}

/// The part of NEAR AI's response kept; serde skips the rest.
#[derive(Deserialize)]
struct Response {
    choices: Vec<Choice>,
    usage: Usage,
}

#[derive(Deserialize)]
struct Choice {
    message: Reply,
    finish_reason: String,
}

#[derive(Deserialize)]
struct Reply {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCall>>,
}

/// An OpenAI-compatible error response; serde skips its `type`, `param` and `code`.
#[derive(Deserialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Deserialize)]
struct ErrorDetail {
    message: String,
}

/// NEAR AI's reply to one request, parsed from its known shape.
fn answer(status: u16, body: &[u8]) -> Result<Answer, Error> {
    if !(200..300).contains(&status) {
        return Err(Error::Status {
            status,
            message: serde_json::from_slice::<ErrorBody>(body)
                .ok()
                .map(|e| e.error.message),
        });
    }
    let response: Response = serde_json::from_slice(body).map_err(|e| Error::Unusable {
        reason: format!("NEAR AI's answer is not a chat completion: {e}"),
    })?;
    let Some(choice) = response.choices.into_iter().next() else {
        return Err(Error::Unusable {
            reason: "NEAR AI's answer has no choices".into(),
        });
    };
    // A reasoning model's content may be only the blank lines after its reasoning.
    let content = choice.message.content.filter(|c| !c.trim().is_empty());
    let tool_calls = choice.message.tool_calls.filter(|c| !c.is_empty());
    if content.is_none() && tool_calls.is_none() {
        let reason = match choice.finish_reason.as_str() {
            "length" => "the model ran out of tokens before answering".into(),
            other => format!("the model answered nothing (finish_reason: {other})"),
        };
        return Err(Error::Unusable { reason });
    }
    Ok(Answer {
        message: Message::Assistant {
            content,
            tool_calls,
        },
        finish_reason: choice.finish_reason,
        usage: response.usage,
    })
}

#[cfg(test)]
mod tests;
