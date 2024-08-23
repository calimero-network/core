use libp2p_identity::PeerId;
use serde::{Deserialize, Serialize};

use crate::context::ContextId;
use crate::hash::Hash;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum NodeEvent {
    Application(ApplicationEvent),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ApplicationEvent {
    pub context_id: ContextId,
    #[serde(flatten)]
    pub payload: ApplicationEventPayload,
}

impl ApplicationEvent {
    #[must_use]
    pub const fn new(context_id: ContextId, payload: ApplicationEventPayload) -> Self {
        Self {
            context_id,
            payload,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "PascalCase")]
#[non_exhaustive]
pub enum ApplicationEventPayload {
    TransactionExecuted(ExecutedTransactionPayload),
    PeerJoined(PeerJoinedPayload),
    OutcomeEvent(OutcomeEventPayload),
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ExecutedTransactionPayload {
    pub hash: Hash,
}

impl ExecutedTransactionPayload {
    #[must_use]
    pub const fn new(hash: Hash) -> Self {
        Self { hash }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PeerJoinedPayload {
    pub peer_id: PeerId,
}

impl PeerJoinedPayload {
    #[must_use]
    pub const fn new(peer_id: PeerId) -> Self {
        Self { peer_id }
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
