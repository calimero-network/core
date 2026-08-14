use std::collections::BTreeMap;

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
    SetEphemeral(SetEphemeralRequest),
    GetEphemeral(GetEphemeralRequest),
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

/// The coarse phase carried in the response — the shared wire type, so the
/// JSON-RPC response and the WebSocket `SyncStatus` event speak the same enum.
pub use calimero_primitives::sync_status::SyncState;

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

// -------------------------------------------- Ephemeral presence types --------------------------------------------

/// Set the caller's local ephemeral-presence slice for a context.
///
/// The author identity is resolved server-side (the node's owned key for the
/// context) — callers never specify it, mirroring the `execute` convention.
/// `state` is the raw presence bytes (e.g. cursor position, typing indicator).
/// Rejected by the handler when `state.len() > EPHEMERAL_MAX_BYTES` (16 384).
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SetEphemeralRequest {
    pub context_id: ContextId,
    pub state: Vec<u8>,
}

impl SetEphemeralRequest {
    #[must_use]
    pub const fn new(context_id: ContextId, state: Vec<u8>) -> Self {
        Self { context_id, state }
    }
}

/// Acknowledgement returned by `set_ephemeral`. Empty body — the call is
/// fire-and-forget from the client's perspective; the JSON-RPC ack keeps
/// the transport uniform and lets the client detect size/auth errors.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SetEphemeralResponse {}

/// Errors that the `set_ephemeral` handler can return to the client.
#[derive(Debug, Deserialize, Serialize, thiserror::Error)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum SetEphemeralError {
    /// The authenticated caller is not a member of the target context, so it
    /// may not publish presence into it.
    #[error("caller is not a member of this context")]
    Unauthorized,
    /// No owned identity found for the context — the node is not a member.
    #[error("no owned identity found for context")]
    NoOwnedIdentity,
    /// The presence slice exceeds the protocol maximum (16 384 bytes).
    #[error("ephemeral slice too large: {size} bytes (max {max})")]
    SliceTooLarge { size: usize, max: usize },
    /// Any other node-level error (key-loading failure, crypto error, etc.).
    #[error("set_ephemeral failed: {0}")]
    InternalError(String),
}

/// Request the current live ephemeral-presence snapshot for a context.
///
/// Returns all authors whose entry has not expired (within
/// `PRESENCE_TTL_MS`). This is the client's initial seed; live deltas then
/// arrive on the event stream (subscribe-then-seed, self-healing within the
/// heartbeat interval).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct GetEphemeralRequest {
    pub context_id: ContextId,
}

impl GetEphemeralRequest {
    #[must_use]
    pub const fn new(context_id: ContextId) -> Self {
        Self { context_id }
    }
}

/// One author's live ephemeral-presence entry, as carried in the
/// [`GetEphemeralResponse`] map (the author is the map key, not a field).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EphemeralEntryValue {
    /// Opaque presence slice. The node never deserializes this — the encoding
    /// is chosen client-side and travels client-to-client.
    pub state: Vec<u8>,
    /// Milliseconds since this author was last heard from, measured on the
    /// responding node.
    ///
    /// **Relative by design.** The underlying `last_seen_ms` is stamped from
    /// the responding node's wall clock; shipping it absolute would force a
    /// caller on another machine to subtract against its own clock, and any
    /// skew between the two would corrupt the result. A relative age needs no
    /// clock agreement.
    ///
    /// Bounded above by the node's presence TTL (7s) for any entry still in
    /// the snapshot, and typically below the heartbeat interval (2.5s) for a
    /// live author. Events delivered over the event stream carry no age — they
    /// are emitted at the moment of change, so a subscriber can stamp receipt
    /// time itself; this field exists because a *snapshot* read cannot tell a
    /// fresh entry from a nearly-expired one.
    pub age_ms: u64,
}

impl EphemeralEntryValue {
    #[must_use]
    pub const fn new(state: Vec<u8>, age_ms: u64) -> Self {
        Self { state, age_ms }
    }
}

/// Response carrying the live ephemeral-presence snapshot for a context,
/// keyed by author.
///
/// Author-keyed rather than a list: the node's awareness store is already a
/// per-author map (author is unique within a context by construction), the
/// events delivered over the event stream are per-author deltas, and every
/// known consumer rebuilds a map immediately. Returning a list would flatten
/// a map only to make each caller reconstruct it, and would leave the snapshot
/// shape mismatched with the delta shape.
///
/// Keys are the author's public key in its string (base58) representation.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct GetEphemeralResponse {
    pub entries: BTreeMap<String, EphemeralEntryValue>,
}

impl GetEphemeralResponse {
    #[must_use]
    pub const fn new(entries: BTreeMap<String, EphemeralEntryValue>) -> Self {
        Self { entries }
    }
}

/// Errors that the `get_ephemeral` handler can return to the client.
#[derive(Debug, Deserialize, Serialize, thiserror::Error)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum GetEphemeralError {
    /// The authenticated caller is not a member of the target context, so it
    /// may not read the (decrypted) presence snapshot for it.
    #[error("caller is not a member of this context")]
    Unauthorized,
    /// Any node-level or internal error.
    #[error("get_ephemeral failed: {0}")]
    InternalError(String),
}

// -------------------------------------------- Validation Implementation --------------------------------------------

impl Validate for SyncStatusRequest {
    fn validate(&self) -> Vec<ValidationError> {
        // `context_id` is a typed, fixed-size identifier — nothing to bound.
        Vec::new()
    }
}

impl Validate for SetEphemeralRequest {
    fn validate(&self) -> Vec<ValidationError> {
        // Size is enforced by the node layer (`EPHEMERAL_MAX_BYTES`); the
        // server handler propagates `SetEphemeralError::SliceTooLarge` if the
        // node rejects. Nothing to bound at the parse layer.
        Vec::new()
    }
}

impl Validate for GetEphemeralRequest {
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
