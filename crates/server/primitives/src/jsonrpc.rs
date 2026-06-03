use calimero_context_client::messages::ExecuteError;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::validation::{Validate, ValidationError};

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum RequestId {
    String(String),
    Number(u64),
    #[default]
    Null,
}

#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub enum Version {
    #[default]
    TwoPointZero,
}

impl Serialize for Version {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::TwoPointZero => serializer.serialize_str("2.0"),
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
            "2.0" => Ok(Self::TwoPointZero),
            _ => Err(de::Error::custom("Invalid JSON-RPC version")),
        }
    }
}

// **************************** request *******************************
#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Request<P> {
    pub jsonrpc: Version,
    pub id: RequestId,
    #[serde(flatten)]
    pub payload: P,
}

impl Request<RequestPayload> {
    #[must_use]
    pub const fn new(jsonrpc: Version, id: RequestId, payload: RequestPayload) -> Self {
        Self {
            jsonrpc,
            id,
            payload,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum RequestPayload {
    Execute(ExecutionRequest),
    SyncStatus(SyncStatusRequest),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Response {
    pub jsonrpc: Version,
    pub id: RequestId,
    #[serde(flatten)]
    pub body: ResponseBody,
}

impl Response {
    #[must_use]
    pub const fn new(jsonrpc: Version, id: RequestId, body: ResponseBody) -> Self {
        Self { jsonrpc, id, body }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::exhaustive_enums,
    reason = "This will never have any other variants"
)]
pub enum ResponseBody {
    Result(ResponseBodyResult),
    Error(ResponseBodyError),
}

#[derive(Debug, Deserialize, Serialize)]
#[expect(
    clippy::exhaustive_structs,
    reason = "This will never have any other fields"
)]
pub struct ResponseBodyResult(pub serde_json::Value);

#[derive(Debug, Deserialize, Serialize, Error)]
#[serde(untagged)]
#[non_exhaustive]
pub enum ResponseBodyError {
    #[error(transparent)]
    ServerError(ServerResponseError),
    #[error("handler error: {0}")]
    HandlerError(serde_json::Value),
}

#[derive(Debug, Deserialize, Serialize, Error)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum ServerResponseError {
    #[error("parse error: {0}")]
    ParseError(String),
    #[error(
        "internal error: {}",
        err.as_ref().map_or_else(|| "<opaque>".to_owned(), |e| e.to_string())
    )]
    InternalError {
        #[serde(skip)]
        err: Option<eyre::Report>,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ExecutionRequest {
    pub context_id: ContextId,
    pub method: String,
    pub args_json: serde_json::Value,
    #[serde(default)]
    pub substitute: Vec<Alias<PublicKey>>,
}

impl ExecutionRequest {
    #[must_use]
    pub const fn new(
        context_id: ContextId,
        method: String,
        args_json: serde_json::Value,
        substitute: Vec<Alias<PublicKey>>,
    ) -> Self {
        Self {
            context_id,
            method,
            args_json,
            substitute,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ExecutionResponse {
    pub output: Option<serde_json::Value>,
}

impl ExecutionResponse {
    #[must_use]
    pub const fn new(output: Option<serde_json::Value>) -> Self {
        Self { output }
    }
}

#[derive(Debug, Deserialize, Serialize, Error)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum ExecutionError {
    #[error("codec error: {message}")]
    SerdeError { message: String },
    #[error("function call error: {0}")]
    FunctionCallError(String),
    #[serde(untagged)]
    #[error(transparent)]
    ExecuteError(ExecuteError),
}

/// Request the current state-sync status of a context. Lets a client that
/// hit `Uninitialized` on `execute` tell whether sync is actively running,
/// waiting for a peer, or wedged — instead of guessing from one opaque error.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SyncStatusRequest {
    pub context_id: ContextId,
}

impl SyncStatusRequest {
    #[must_use]
    pub const fn new(context_id: ContextId) -> Self {
        Self { context_id }
    }
}

/// Coarse, wire-facing sync phase. Serialized internally-tagged as
/// `{ "state": "syncing" }`, with `backingOff` additionally carrying
/// `retryInSecs`. A typed enum (rather than a bare string) keeps `retry_in_secs`
/// structurally bound to the one state it applies to and lets callers match
/// exhaustively. Not `#[non_exhaustive]` so the server handler can construct it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum SyncState {
    /// Context is settled — no sync in flight, nothing pending.
    Idle,
    /// Not yet initialized and nothing is actively syncing: typically waiting
    /// for a co-member peer to appear to sync the initial state from.
    WaitingForPeers,
    /// A sync attempt is currently in flight.
    Syncing,
    /// The last attempt failed and the next retry is gated behind backoff.
    BackingOff {
        /// Estimated seconds until the next retry is eligible.
        retry_in_secs: u64,
    },
}

/// Sync-status response. `sync_state` carries the coarse phase; a non-zero
/// `failure_count` with `last_error` set is the "stuck" signal. Note
/// `is_initialized` and `sync_state` are orthogonal: an already-initialized
/// context may still report `syncing` while it catches up on later deltas.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SyncStatusResponse {
    pub context_id: ContextId,
    /// `true` once the context has a non-zero root hash, i.e. initial state
    /// has been adopted and `execute` will no longer return `Uninitialized`.
    pub is_initialized: bool,
    /// Coarse sync phase.
    pub sync_state: SyncState,
    /// Consecutive failed sync attempts (0 when healthy).
    pub failure_count: u32,
    /// Most recent sync error, if the last attempt failed. Carries the reason
    /// behind a `backingOff` state (e.g. "No peers to sync with").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl SyncStatusResponse {
    #[must_use]
    pub const fn new(
        context_id: ContextId,
        is_initialized: bool,
        sync_state: SyncState,
        failure_count: u32,
        last_error: Option<String>,
    ) -> Self {
        Self {
            context_id,
            is_initialized,
            sync_state,
            failure_count,
            last_error,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Error)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum SyncStatusError {
    #[error("context not found")]
    ContextNotFound,
}

// -------------------------------------------- Validation Implementation --------------------------------------------

impl Validate for SyncStatusRequest {
    fn validate(&self) -> Vec<ValidationError> {
        // `context_id` is a typed, fixed-size identifier — nothing to bound.
        Vec::new()
    }
}

impl Validate for ExecutionRequest {
    fn validate(&self) -> Vec<ValidationError> {
        use crate::validation::helpers::{
            validate_collection_size, validate_json_size, validate_method_name,
        };
        use crate::validation::{MAX_ARGS_JSON_SIZE, MAX_SUBSTITUTE_ALIASES};

        let mut errors = Vec::new();

        // Validate method name
        if let Some(e) = validate_method_name(&self.method, "method") {
            errors.push(e);
        }

        // Validate args_json size
        if let Some(e) = validate_json_size(&self.args_json, "args_json", MAX_ARGS_JSON_SIZE) {
            errors.push(e);
        }

        // Validate substitute aliases count
        if let Some(e) =
            validate_collection_size(&self.substitute, "substitute", MAX_SUBSTITUTE_ALIASES)
        {
            errors.push(e);
        }

        errors
    }
}
