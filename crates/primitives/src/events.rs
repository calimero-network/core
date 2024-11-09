use serde::{Deserialize, Serialize};

use crate::context::ContextId;
use crate::hash::Hash;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum NodeEvent {
    Context(ContextEvent),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ContextEvent {
    pub context_id: ContextId,
    #[serde(flatten)]
    pub payload: ContextEventPayload,
}

impl ContextEvent {
    #[must_use]
    pub const fn new(context_id: ContextId, payload: ContextEventPayload) -> Self {
        Self {
            context_id,
            payload,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "PascalCase")]
#[non_exhaustive]
#[expect(variant_size_differences, reason = "fine for now")]
pub enum ContextEventPayload {
    StateMutation(StateMutationPayload),
    ExecutionEvent(ExecutionEventPayload),
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct StateMutationPayload {
    pub new_root: Hash,
}

impl StateMutationPayload {
    #[must_use]
    pub const fn new(new_root: Hash) -> Self {
        Self { new_root }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ExecutionEvent {
    pub kind: String,
    pub data: Vec<u8>,
}

impl ExecutionEvent {
    #[must_use]
    pub const fn new(kind: String, data: Vec<u8>) -> Self {
        Self { kind, data }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ExecutionEventPayload {
    pub events: Vec<ExecutionEvent>,
}

impl ExecutionEventPayload {
    #[must_use]
    pub const fn new(events: Vec<ExecutionEvent>) -> Self {
        Self { events }
    }
}
