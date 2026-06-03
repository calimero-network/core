use serde::{Deserialize, Serialize};

use crate::context::ContextId;
use crate::hash::Hash;
use crate::sync_status::SyncState;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum NodeEvent {
    Context(ContextEvent),
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
    /// Live sync-status update, pushed to subscribers as the sync run-loop
    /// changes phase (and as snapshot pages arrive). Lets a client waiting on
    /// initial state watch progress instead of polling `sync_status`.
    SyncStatus(SyncStatusPayload),
}

/// Payload of a [`ContextEventPayload::SyncStatus`] event. Mirrors the fields
/// of the `sync_status` JSON-RPC response that the run-loop knows; `is_initialized`
/// is deliberately omitted (it's a context-layer fact, not a sync-phase one —
/// a client reads it from the RPC or infers initialization from the first
/// [`ContextEventPayload::StateMutation`]).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatusPayload {
    pub sync_state: SyncState,
    pub failure_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
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
