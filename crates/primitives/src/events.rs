use serde::{Deserialize, Serialize};

use crate::context::ContextId;
use crate::hash::Hash;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum NodeEvent {
    Context(ContextEvent),
    Sync(SyncEvent),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncEvent {
    pub context_id: ContextId,
    #[serde(flatten)]
    pub payload: SyncEventPayload,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "PascalCase")]
pub enum SyncEventPayload {
    /// Sync operation started for a context
    SyncStarted,
    /// Sync operation completed successfully
    SyncCompleted {
        /// Which sync protocol was used
        protocol: String,
        /// How long the sync took
        duration_ms: u64,
    },
    /// Sync operation failed
    SyncFailed {
        /// Error message
        error: String,
        /// Whether this was a retry
        is_retry: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextEvent {
    pub context_id: ContextId,
    #[serde(flatten)]
    pub payload: ContextEventPayload,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "PascalCase")]
#[allow(variant_size_differences, reason = "fine for now")]
pub enum ContextEventPayload {
    StateMutation(StateMutationPayload),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateMutationPayload {
    pub new_root: Hash,
    pub events: Option<Vec<ExecutionEvent>>,
}

impl StateMutationPayload {
    #[must_use]
    pub const fn with_root_and_events(new_root: Hash, events: Vec<ExecutionEvent>) -> Self {
        Self {
            new_root,
            events: Some(events),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExecutionEvent {
    pub kind: String,
    pub data: Vec<u8>,
    pub handler: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionXCall {
    pub target_context_id: ContextId,
    pub function: String,
    pub params: Vec<u8>,
}
