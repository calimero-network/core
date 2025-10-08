use serde::{Deserialize, Serialize};

use crate::context::ContextId;
use crate::hash::Hash;

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
#[expect(variant_size_differences, reason = "fine for now")]
pub enum ContextEventPayload {
    StateMutation(StateMutationPayload),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateMutationPayload {
    pub new_root: Option<Hash>,
    pub events: Vec<ExecutionEvent>,
}

impl StateMutationPayload {
    #[must_use]
    pub const fn with_root_and_events(new_root: Option<Hash>, events: Vec<ExecutionEvent>) -> Self {
        Self { new_root, events }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExecutionEvent {
    pub kind: String,
    pub data: Vec<u8>,
}

// ExecutionEventPayload removed; events are now embedded in StateMutationPayload
