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
    OutcomeEvent(OutcomeEventPayload),
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
pub struct OutcomeEvent {
    pub kind: String,
    pub data: Vec<u8>,
}

impl OutcomeEvent {
    #[must_use]
    pub const fn new(kind: String, data: Vec<u8>) -> Self {
        Self { kind, data }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct OutcomeEventPayload {
    pub events: Vec<OutcomeEvent>,
}

impl OutcomeEventPayload {
    #[must_use]
    pub const fn new(events: Vec<OutcomeEvent>) -> Self {
        Self { events }
    }
}
